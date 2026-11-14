#!/usr/bin/env node
'use strict';
// SPDX-License-Identifier: GPL-3.0-or-later
// Optional OTT home bridge. Home Assistant credentials stay on this host.
const http = require('node:http');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

// Admit bounded regular files without following a leaf or parent symlink.
// Verify the same descriptor and directory identities after the bounded read.
function directoryChain(directory) {
  const absolute = path.resolve(directory);
  let current = path.parse(absolute).root;
  const rows = [];
  for (const part of ['', ...absolute.slice(current.length).split(path.sep).filter(Boolean)]) {
    if (part) current = path.join(current, part);
    const stat = fs.lstatSync(current);
    if (stat.isSymbolicLink() || !stat.isDirectory()) throw new Error('invalid_asset');
    rows.push({file:current,dev:stat.dev,ino:stat.ino});
  }
  return rows;
}
function verifyDirectories(rows) {
  for (const row of rows) {
    const stat = fs.lstatSync(row.file);
    if (stat.isSymbolicLink() || !stat.isDirectory() || stat.dev !== row.dev || stat.ino !== row.ino)
      throw new Error('invalid_asset');
  }
}
function readPublicFile(file, limit = 1024 * 1024) {
  const absolute = path.resolve(file);
  const parents = directoryChain(path.dirname(absolute));
  const before = fs.lstatSync(absolute);
  if (before.isSymbolicLink() || !before.isFile() || before.nlink !== 1 || before.size > limit) throw new Error('invalid_asset');
  const flags = fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0) | (fs.constants.O_NONBLOCK || 0);
  const fd = fs.openSync(absolute, flags);
  try {
    const initial = fs.fstatSync(fd);
    if (!initial.isFile() || initial.nlink !== 1 || initial.dev !== before.dev || initial.ino !== before.ino || initial.size !== before.size)
      throw new Error('invalid_asset');
    const bytes = Buffer.alloc(initial.size + 1);
    let count = 0, read;
    while (count < bytes.length && (read = fs.readSync(fd, bytes, count, bytes.length - count, null)) > 0) count += read;
    const after = fs.fstatSync(fd), leaf = fs.lstatSync(absolute);
    if (count !== initial.size || !after.isFile() || after.nlink !== 1 || after.dev !== initial.dev || after.ino !== initial.ino ||
        after.size !== initial.size || after.mtimeMs !== initial.mtimeMs || after.ctimeMs !== initial.ctimeMs ||
        leaf.isSymbolicLink() || !leaf.isFile() || leaf.nlink !== 1 || leaf.dev !== initial.dev || leaf.ino !== initial.ino)
      throw new Error('invalid_asset');
    verifyDirectories(parents);
    return bytes.subarray(0, count);
  } finally { fs.closeSync(fd); }
}


const SERVICES = new Set(['light.turn_on','light.turn_off','light.toggle',
  'switch.turn_on','switch.turn_off','switch.toggle','scene.turn_on','script.turn_on',
  'cover.open_cover','cover.close_cover','cover.stop_cover']);
const ID = /^[a-z0-9_-]{1,64}$/;
const ENTITY = /^[a-z][a-z0-9_]*\.[a-z0-9_]{1,128}$/;
function invalid() { throw new Error('invalid_configuration'); }
function text(value, max = 128) {
  if (typeof value !== 'string' || !value.trim() || value.length > max || /[\x00-\x1f]/.test(value)) invalid();
  return value;
}
function origin(value, localHttp = false) {
  let url; try { url = new URL(value); } catch { invalid(); }
  if (url.username || url.password || url.search || url.hash || url.pathname !== '/') invalid();
  if (url.protocol !== 'https:' && !(localHttp && url.protocol === 'http:')) invalid();
  return url.origin;
}
function configuration(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw) || raw.version !== 1) invalid();
  const haOrigin = origin(raw.homeAssistant?.url, raw.allowPlainHttp === true);
  const haToken = text(raw.homeAssistant?.token, 4096);
  const accessKey = text(raw.accessKey, 256);
  if (haToken === accessKey || /^REPLACE/.test(haToken) || /^REPLACE/.test(accessKey) || Buffer.byteLength(accessKey) < 32 || !/^[A-Za-z0-9_-]+$/.test(accessKey)) invalid();
  const origins = (raw.allowedOrigins || []).map(x => x === 'null' && raw.allowPackagedOrigin === true ? x : origin(x, raw.allowPlainHttp === true));
  if (origins.length > 16 || !Array.isArray(raw.entities) || raw.entities.length > 32 ||
      !Array.isArray(raw.actions) || raw.actions.length > 16 || !Array.isArray(raw.apps || []) || raw.apps?.length > 16) invalid();
  const used = new Set();
  function identity(item) {
    if (!item || !ID.test(item.id) || used.has(item.id)) invalid();
    used.add(item.id); text(item.label);
  }
  const entities = raw.entities.map(item => {
    identity(item); if (!ENTITY.test(item.entity) || !['status','temperature','weather','energy'].includes(item.kind)) invalid();
    return {id:item.id,label:item.label,entity:item.entity,kind:item.kind};
  });
  const actions = raw.actions.map(item => {
    identity(item);
    if (!SERVICES.has(item.service) || !ENTITY.test(item.entity) || item.entity.split('.')[0] !== item.service.split('.')[0]) invalid();
    return {id:item.id,label:item.label,entity:item.entity,service:item.service};
  });
  const apps = (raw.apps || []).map(item => {
    identity(item); if (!/^media_player\.[a-z0-9_]{1,128}$/.test(item.entity) || !['app','hdmi','tv'].includes(item.kind)) invalid();
    return {id:item.id,label:item.label,entity:item.entity,kind:item.kind,source:text(item.source)};
  });
  return {haOrigin,haToken,accessKey,origins,entities,actions,apps};
}

class HomeAssistant {
  constructor(config, ports = {}) {
    this.config = config; this.WebSocket = ports.WebSocket || globalThis.WebSocket;
    if (typeof this.WebSocket !== 'function') throw new Error('websocket_runtime_required');
    this.now = ports.now || Date.now; this.schedule = ports.schedule || setTimeout;
    this.cancel = ports.cancel || clearTimeout; this.socket = null; this.generation = 0;
    this.ready = false; this.stopped = true; this.states = new Map(); this.pending = new Map();
    this.revisions = new Map(); this.serial = 0; this.refreshPending = null;
    this.nextId = 1; this.retry = 0; this.retryTimer = null; this.handshakeTimer = null; this.lastMessage = 0;
    this.watched = new Set([...config.entities,...config.actions,...config.apps].map(x => x.entity));
  }
  start() { if (!this.stopped) return; this.stopped = false; this.connect(); }
  stop() {
    this.stopped = true; this.generation++; this.ready = false; this.states.clear(); this.refreshPending = null;
    if (this.retryTimer) this.cancel(this.retryTimer); this.retryTimer = null;
    if (this.handshakeTimer) this.cancel(this.handshakeTimer); this.handshakeTimer = null;
    const socket = this.socket; this.socket = null; if (socket) { try { socket.close(); } catch {} }
    this.rejectPending();
  }
  rejectPending() {
    for (const item of this.pending.values()) { this.cancel(item.timer); item.reject(new Error('home_unavailable')); }
    this.pending.clear();
  }
  connect() {
    if (this.stopped) return;
    const generation = ++this.generation;
    const endpoint = this.config.haOrigin.replace(/^http/, 'ws') + '/api/websocket';
    let socket; try { socket = new this.WebSocket(endpoint); } catch { this.reconnect(generation); return; }
    this.socket = socket;
    const active = () => !this.stopped && this.socket === socket && this.generation === generation;
    const handshake = this.handshakeTimer = this.schedule(() => { if (active() && !this.ready) this.disconnect(socket,generation); },8000);
    socket.addEventListener('message', event => {
      if (!active() || typeof event.data !== 'string' || event.data.length > 4 * 1024 * 1024) return;
      let msg; try { msg = JSON.parse(event.data); } catch { return; }
      if (!msg || typeof msg !== 'object' || Array.isArray(msg)) return;
      this.lastMessage = this.now();
      if (msg.type === 'auth_required') {
        try { socket.send(JSON.stringify({type:'auth',access_token:this.config.haToken})); }
        catch { this.disconnect(socket,generation); }
      }
      else if (msg.type === 'auth_ok') {
        this.cancel(handshake); this.handshakeTimer = null;
        this.ready = true; this.retry = 0;
        this.request('subscribe_events',{event_type:'state_changed'}).catch(() => {});
        this.refresh().catch(() => {});
      } else if (msg.type === 'auth_invalid') this.disconnect(socket,generation);
      else if (msg.type === 'result' && this.pending.has(msg.id)) {
        const item = this.pending.get(msg.id); this.pending.delete(msg.id); this.cancel(item.timer);
        if (msg.success) item.resolve(msg.result); else item.reject(new Error('home_action_failed'));
      } else if (msg.type === 'event') {
        const state = msg.event?.data?.new_state; const entity = msg.event?.data?.entity_id;
        if (this.watched.has(entity)) {
          this.revisions.set(entity,++this.serial);
          if (state) this.states.set(entity,state); else this.states.delete(entity);
        }
      }
    });
    const closed = () => { this.cancel(handshake); if (active()) this.disconnect(socket,generation); };
    socket.addEventListener('close', closed); socket.addEventListener('error', () => { if (active()) this.disconnect(socket,generation); });
  }
  disconnect(socket,generation) {
    if (this.stopped || socket !== this.socket || generation !== this.generation) return;
    if (this.handshakeTimer) this.cancel(this.handshakeTimer); this.handshakeTimer = null;
    this.ready = false; this.states.clear(); this.socket = null; this.refreshPending = null; this.rejectPending();
    try { socket.close(); } catch {}
    this.reconnect(generation);
  }
  reconnect(generation) {
    if (this.stopped || generation !== this.generation || this.retryTimer) return;
    const delay = Math.min(30000, 1000 * 2 ** Math.min(this.retry++,5));
    this.retryTimer = this.schedule(() => { this.retryTimer = null; if (!this.stopped && generation === this.generation) this.connect(); },delay);
  }
  request(type, fields = {}) {
    if (!this.ready || !this.socket || this.pending.size >= 8) return Promise.reject(new Error('home_unavailable'));
    const id = this.nextId++; const socket = this.socket; const generation = this.generation;
    return new Promise((resolve,reject) => {
      const timer = this.schedule(() => { this.pending.delete(id); reject(new Error('home_unavailable')); this.disconnect(socket,generation); },8000);
      this.pending.set(id,{resolve,reject,timer});
      try { socket.send(JSON.stringify({id,type,...fields})); }
      catch { this.pending.delete(id); this.cancel(timer); reject(new Error('home_unavailable')); this.disconnect(socket,generation); }
    });
  }
  refresh() {
    const generation = this.generation;
    if (this.refreshPending?.generation === generation) return this.refreshPending.promise;
    const pending = {generation,promise:null};
    this.refreshPending = pending;
    const serial = this.serial;
    pending.promise = (async () => {
      const states = await this.request('get_states');
      if (generation !== this.generation || !this.ready || !Array.isArray(states)) throw new Error('home_unavailable');
      const next = new Map(); for (const state of states) if (state && this.watched.has(state.entity_id)) next.set(state.entity_id,state);
      // Events (including deletion) can overtake this one shared snapshot.
      for (const [entity, revision] of this.revisions) if (revision > serial) {
        if (this.states.has(entity)) next.set(entity,this.states.get(entity)); else next.delete(entity);
      }
      this.states = next;
    })().finally(() => { if (this.refreshPending === pending) this.refreshPending = null; });
    return pending.promise;
  }
  available() { return this.ready && this.now() - this.lastMessage >= 0 && this.now() - this.lastMessage <= 45000; }
  snapshot() {
    const available = this.available();
    const bounded = value => typeof value === 'string' || typeof value === 'number' ? String(value).slice(0,256) : '';
    return {version:1,available,entities:this.config.entities.map(item => {
      const state = available ? this.states.get(item.entity) : null;
      return {id:item.id,label:item.label,kind:item.kind,state:state ? bounded(state.state) : 'unavailable',unit:state ? bounded(state.attributes?.unit_of_measurement) : ''};
    }),actions:this.config.actions.map(({id,label}) => ({id,label})),apps:this.config.apps.map(({id,label,kind}) => ({id,label,kind}))};
  }
  async action(id) {
    if (!this.available()) throw new Error('home_unavailable');
    const control = this.config.actions.find(x => x.id === id);
    const app = this.config.apps.find(x => x.id === id);
    if (!control && !app) throw new Error('unknown_action');
    if (control) {
      const [domain,service] = control.service.split('.');
      await this.request('call_service',{domain,service,target:{entity_id:control.entity}});
    } else {
      const sources = this.states.get(app.entity)?.attributes?.source_list;
      if (!Array.isArray(sources) || !sources.includes(app.source)) throw new Error('source_unavailable');
      await this.request('call_service',{domain:'media_player',service:'select_source',target:{entity_id:app.entity},service_data:{source:app.source}});
    }
    return {version:1,status:'requested'};
  }
}

function createHandler(config, home, options = {}) {
  const secret = Buffer.from(config.accessKey);
  let inFlight = false;
  return async (req,res) => {
    res.setHeader('Cache-Control','no-store'); res.setHeader('Content-Type','application/json');
    res.setHeader('X-Content-Type-Options','nosniff');
    const reply = (code,data) => { res.statusCode = code; res.end(JSON.stringify(data)); };
    const assets = {'/':'index.html','/index.html':'index.html','/app.js':'app.js','/core.js':'core.js','/webos-host.js':'webos-host.js','/style.css':'style.css'};
    if (options.dashboardRoot && req.method === 'GET' && assets[req.url]) {
      try {
        const name = assets[req.url];
        const mime = name.endsWith('.js') ? 'text/javascript' : name.endsWith('.css') ? 'text/css' : 'text/html';
        const body = readPublicFile(path.join(options.dashboardRoot,name));
        res.setHeader('Content-Type',mime+'; charset=utf-8');
        res.setHeader('Content-Security-Policy',"default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self' https: http:; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'self' " + config.origins.filter(x => x !== 'null').join(' '));
        return res.end(body);
      } catch { return reply(404,{error:'not_found'}); }
    }
    const suppliedOrigin = req.headers.origin;
    if (suppliedOrigin && !config.origins.includes(suppliedOrigin)) return reply(403,{error:'origin_rejected'});
    if (suppliedOrigin) { res.setHeader('Access-Control-Allow-Origin',suppliedOrigin); res.setHeader('Vary','Origin'); }
    if (req.method === 'OPTIONS') {
      res.setHeader('Access-Control-Allow-Methods','GET, POST, OPTIONS'); res.setHeader('Access-Control-Allow-Headers','Authorization, Content-Type');
      res.statusCode = 204; return res.end();
    }
    const value = req.headers.authorization;
    const token = typeof value === 'string' && value.startsWith('Bearer ') ? Buffer.from(value.slice(7)) : Buffer.alloc(0);
    if (token.length !== secret.length || !crypto.timingSafeEqual(token,secret)) return reply(401,{error:'unauthorized'});
    if (req.method === 'GET' && req.url === '/api/home') {
      try { await home.refresh(); return reply(200,home.snapshot()); } catch { return reply(503,{error:'home_unavailable'}); }
    }
    if (req.method !== 'POST' || req.url !== '/api/action') return reply(404,{error:'not_found'});
    if (inFlight) return reply(409,{error:'busy'});
    if (req.headers['content-type']?.split(';')[0].trim() !== 'application/json') return reply(415,{error:'invalid_request'});
    inFlight = true;
    try {
      let body = ''; for await (const chunk of req) { body += chunk; if (Buffer.byteLength(body) > 1024) return reply(413,{error:'invalid_request'}); }
      let data; try { data = JSON.parse(body); } catch { return reply(400,{error:'invalid_request'}); }
      if (!data || Object.keys(data).join(',') !== 'id' || !ID.test(data.id)) return reply(400,{error:'invalid_request'});
      return reply(200,await home.action(data.id));
    } catch (error) { return reply(503,{error:['unknown_action','source_unavailable'].includes(error.message) ? error.message : 'home_unavailable'}); }
    finally { inFlight = false; }
  };
}

if (require.main === module) {
  let home, server;
  function cleanup() { if (home) home.stop(); if (server) server.close(); }
  try {
    const file = process.env.OTT_HOME_CONFIG;
    if (!file || (fs.statSync(file).mode & 0o077)) throw new Error('private_configuration_required');
    const config = configuration(JSON.parse(fs.readFileSync(file,'utf8')));
    const host = process.env.OTT_HOME_BIND || '127.0.0.1'; const port = Number(process.env.OTT_HOME_PORT || '8790');
    if (!Number.isInteger(port) || port < 1 || port > 65535) invalid();
    home = new HomeAssistant(config);
    const dashboardRoot = process.env.OTT_HOME_ASSETS || require('node:path').join(__dirname,'../dashboard');
    server = http.createServer(createHandler(config,home,{dashboardRoot}));
    server.requestTimeout = 10000; server.headersTimeout = 10000;
    server.once('error',() => { cleanup(); console.error('OTT home bridge: listener failed'); process.exitCode = 1; });
    server.listen(port,host,() => { home.start(); console.log('OTT home bridge listening; use a trusted HTTPS reverse proxy for remote clients.'); });
    for (const signal of ['SIGINT','SIGTERM']) process.once(signal,cleanup);
  } catch { cleanup(); console.error('OTT home bridge: configuration or startup failed'); process.exitCode = 1; }
}
module.exports = {configuration,HomeAssistant,createHandler};
