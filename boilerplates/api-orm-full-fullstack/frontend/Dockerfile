# Frontend nginx — sirve HTML/CSS/JS vanilla + proxy a la API.
#
# Sin build step (no npm, no webpack, no node_modules). Cada
# `index.html` es self-contained con `<style>` y `<script>` inline
# o referencias a CDN. El gen entero pesa <100 KB.
#
# El nginx además proxy-ea:
#   /api/*  → http://api:3000/*   (HTTP)
#   /ws/*   → http://api:3000/*   (WebSocket con Upgrade header)
#
# Eso hace que el browser haga requests SAME-ORIGIN al backend, sin
# CORS preflight ni gap del WS auth con token en query param.

FROM nginx:alpine

COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY html/ /usr/share/nginx/html/

EXPOSE 8080
