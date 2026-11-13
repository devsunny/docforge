# Full image: native engine plus the Linux-capable external toolchain.
FROM rust:1.90-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY catalog ./catalog
COPY docs ./docs
RUN cargo build --release --locked -p docforge-server

FROM debian:bookworm-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libreoffice-writer \
    libreoffice-calc \
    libreoffice-impress \
    pandoc \
    poppler-utils \
    ghostscript \
    imagemagick \
    tesseract-ocr \
    tesseract-ocr-eng \
    calibre \
    libheif-examples \
    p7zip-full \
    djvulibre-bin \
    libgxps-utils \
    antiword \
    qpdf \
    fonts-dejavu-core \
    fonts-liberation \
    && rm -rf /var/lib/apt/lists/*

# ImageMagick's default Debian policy disables PDF for security reasons.
# DocForge invokes it on user-provided files only; enable read/write so
# PDF→image routes work. PDF→text still goes through Poppler.
RUN if [ -f /etc/ImageMagick-6/policy.xml ]; then \
      sed -i 's/rights="none" pattern="PDF"/rights="read|write" pattern="PDF"/' /etc/ImageMagick-6/policy.xml || true; \
    fi

RUN useradd --system --uid 10001 --create-home --home-dir /data --shell /usr/sbin/nologin docforge \
    && mkdir -p /data \
    && chown -R docforge:docforge /data

COPY --from=builder /build/target/release/docforge /usr/local/bin/docforge

ENV DATA_DIR=/data \
    BIND_ADDR=0.0.0.0:8080 \
    PUBLIC_BASE_URL=http://localhost:8080 \
    AUTH_ISSUER=http://localhost:8080 \
    AUTH_DEMO_MODE=true \
    RUST_LOG=docforge=info,tower_http=info

USER docforge
WORKDIR /data
EXPOSE 8080
VOLUME ["/data"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["docforge"]
CMD ["serve"]
