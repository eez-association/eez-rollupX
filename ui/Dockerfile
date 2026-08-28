# syntax=docker/dockerfile:1
FROM node:22-alpine AS build
WORKDIR /app/ui

COPY ui/package.json ui/package-lock.json ./
RUN npm ci

COPY ui ./
RUN npm run build

FROM nginx:1.27-alpine
ARG EEZ_UI_PROTOCOL_COMMIT=development
ENV EEZ_UI_PROTOCOL_COMMIT=$EEZ_UI_PROTOCOL_COMMIT
COPY --from=build /app/ui/dist /usr/share/nginx/html
COPY ui/config.template.json /opt/eez/config.template.json
COPY ui/docker/default.conf.template /etc/nginx/templates/default.conf.template
COPY ui/docker/30-eez-runtime-config.sh /docker-entrypoint.d/30-eez-runtime-config.sh
RUN chmod +x /docker-entrypoint.d/30-eez-runtime-config.sh
EXPOSE 8080
