FROM debian:bookworm-slim

LABEL org.opencontainers.image.source=https://github.com/cpg314/pv_inspect
LABEL org.opencontainers.image.licenses=MIT

COPY target-cross/x86_64-unknown-linux-gnu/release/pv_inspect /usr/bin/pv_inspect

CMD ["/usr/bin/pv_inspect"]
