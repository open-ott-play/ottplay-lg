FROM node:24-alpine
WORKDIR /opt/ott-home
COPY bridge/server.cjs /opt/ott-home/bridge/server.cjs
COPY dashboard/index.html dashboard/style.css dashboard/core.js dashboard/webos-host.js dashboard/app.js /opt/ott-home/dashboard/
COPY LICENSE /opt/ott-home/LICENSE
USER node
ENV OTT_HOME_BIND=0.0.0.0 OTT_HOME_PORT=8790 OTT_HOME_CONFIG=/run/ott-home/config.json
EXPOSE 8790
CMD ["node", "bridge/server.cjs"]
