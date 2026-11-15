'use strict';
// No timers, I/O, credentials or device operations in these decisions.
exports.allowed=function(control,params,boot,wall){
  return !!(control && params && control.session===params.session && control.boot===boot && control.enabled && !control.paused &&
    (control.mode==='trial' ? control.deadline>wall && control.deadline-control.started<=120000 : control.mode==='enabled' && control.accepted===true));
};
exports.acceptance=function(s,p){return !!(p && p.acceptTestedControls===true && s.lastTrial && s.lastTrial.active===true && p.testedTrialSession===s.lastTrial.session);};
exports.seconds=function(n){if(typeof n!=='number'||n%1!==0||n<1||n>120)throw Error('trialSeconds must be an integer in 1..120');return n;};
exports.identity=function(o,uid,gid,groups){return uid!==0 && uid===o.nativeUid && (gid===o.sharedGid || groups.indexOf(o.sharedGid)!==-1);};
