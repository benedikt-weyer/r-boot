#!/usr/bin/env python3
"""Tiny caching reverse proxy for dl-cdn.alpinelinux.org.

Used by scripts/run-linux-qemu so the guest's runtime modloop/apk fetches
hit a local on-disk cache instead of re-downloading from the real Alpine
CDN on every QEMU boot. Reachable from the guest at 10.0.2.2 (QEMU
usermode networking maps that address to the host).

Usage: alpine-proxy.py <port> <cache-dir>
       port 0 picks a free port; the chosen port is printed to stdout.
"""

import http.server
import os
import shutil
import sys
import urllib.request

UPSTREAM = "https://dl-cdn.alpinelinux.org"


def make_handler(cache_dir: str) -> type[http.server.BaseHTTPRequestHandler]:
    class CachingProxyHandler(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            path = self.path.lstrip("/")
            if not path or ".." in path.split("/"):
                self.send_error(400)
                return

            cache_path = os.path.join(cache_dir, path)
            if not os.path.isfile(cache_path):
                os.makedirs(os.path.dirname(cache_path), exist_ok=True)
                part_path = cache_path + ".part"
                try:
                    with urllib.request.urlopen(f"{UPSTREAM}/{path}", timeout=60) as resp:
                        with open(part_path, "wb") as f:
                            shutil.copyfileobj(resp, f)
                    os.replace(part_path, cache_path)
                except Exception as exc:  # noqa: BLE001 - report upstream to guest
                    if os.path.exists(part_path):
                        os.remove(part_path)
                    self.send_error(502, str(exc))
                    return

            self.send_response(200)
            self.send_header("Content-Length", str(os.path.getsize(cache_path)))
            self.end_headers()
            with open(cache_path, "rb") as f:
                shutil.copyfileobj(f, self.wfile)

        def log_message(self, fmt: str, *args: object) -> None:
            print(f"alpine-proxy: {fmt % args}", file=sys.stderr)

    return CachingProxyHandler


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    cache_dir = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.getcwd(), ".cache/alpine-repo")
    os.makedirs(cache_dir, exist_ok=True)

    server = http.server.ThreadingHTTPServer(("0.0.0.0", port), make_handler(cache_dir))
    print(server.server_address[1], flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
