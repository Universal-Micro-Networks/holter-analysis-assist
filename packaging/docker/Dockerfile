# packaging/docker/Dockerfile — DockerBuilder (packaging-distribution)
#
# Linux x86_64 CPU-default runtime image. Copies the verified staging tree
# (embedded holter-http-api); does NOT rebuild or inject raw models inside the image.
#
# Build (preferred — wires .dockerignore into staging context, linux/amd64):
#   ./packaging/scripts/build-docker.sh \
#     --staging packaging/out/staging/linux \
#     --tag holter-http-api:<version>-cpu
#
# Manual equivalent (after prepare-staging + verify-artifact):
#   cp packaging/docker/.dockerignore packaging/out/staging/linux/.dockerignore
#   docker build \
#     --platform linux/amd64 \
#     -f packaging/docker/Dockerfile \
#     -t holter-http-api:<version>-cpu \
#     packaging/out/staging/linux
#
# Tag policy: CPU is the required default distribution variant.
# Example: holter-http-api:1.0.0-cpu
# CUDA (if offered later) MUST use a separate tag/image, never this default.
#
# Config path (upstream http-api):
#   --config / HOLTER_HTTP_INI (default config/http.ini)
#   Copy/edit the sample: http.ini.example → config/http.ini (set license server_url + bind)
#
# Listen port: sample ini uses bind=0.0.0.0:8080 → map host port to container 8080.

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Staging layout (packaging/out/staging/linux): bin/, NOTICE, http.ini.example
COPY bin/holter-http-api /app/bin/holter-http-api
COPY NOTICE /app/NOTICE
COPY http.ini.example /app/http.ini.example

RUN mkdir -p /app/config \
    && cp /app/http.ini.example /app/config/http.ini.example \
    && chmod 755 /app/bin/holter-http-api

# Operators should mount or place runtime ini at this path (or override via env/flag).
ENV HOLTER_HTTP_INI=/app/config/http.ini

EXPOSE 8080

ENTRYPOINT ["/app/bin/holter-http-api"]
CMD ["--config", "/app/config/http.ini"]
