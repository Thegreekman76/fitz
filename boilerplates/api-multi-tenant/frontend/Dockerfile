# Frontend nginx — sirve HTML/CSS/JS vanilla + proxy a la API.
#
# Sin build step (no npm, no webpack, no node_modules). Cada
# `*.html` es self-contained con `<style>` y `<script>` inline.
# Gen entero pesa <50 KB.
#
# El nginx además proxy-ea `/api/*` → `http://api:3000/*` para que
# el browser haga requests SAME-ORIGIN al backend (sin CORS).

FROM nginx:alpine

COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY html/ /usr/share/nginx/html/

EXPOSE 8080
