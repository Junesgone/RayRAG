#!/usr/bin/env python3
"""Expose a Cloudflare WebSocket TCP tunnel as an SSH ProxyCommand stream.

Usage:
    ssh -o "ProxyCommand=python3 scripts/ssh-websocket-proxy.py %h %p" user@host

The proxy contains no credentials. SSH authentication and host-key checking
remain the responsibility of the SSH client.
"""

from __future__ import annotations

import base64
import os
import socket
import ssl
import struct
import sys
import threading


class TunnelSocket:
    def __init__(self, sock: ssl.SSLSocket, buffered: bytes = b"") -> None:
        self.sock = sock
        self.buffered = bytearray(buffered)
        self.send_lock = threading.Lock()

    def recv(self, size: int) -> bytes:
        if self.buffered:
            chunk = bytes(self.buffered[:size])
            del self.buffered[:size]
            return chunk
        return self.sock.recv(size)

    def sendall(self, data: bytes) -> None:
        with self.send_lock:
            self.sock.sendall(data)

    def close(self) -> None:
        self.sock.close()


def recv_exact(sock: TunnelSocket, size: int) -> bytes:
    data = bytearray()
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise EOFError("WebSocket tunnel closed")
        data.extend(chunk)
    return bytes(data)


def send_frame(sock: TunnelSocket, payload: bytes, opcode: int = 0x2) -> None:
    first = 0x80 | opcode
    length = len(payload)
    if length < 126:
        header = bytes((first, 0x80 | length))
    elif length <= 0xFFFF:
        header = bytes((first, 0x80 | 126)) + struct.pack("!H", length)
    else:
        header = bytes((first, 0x80 | 127)) + struct.pack("!Q", length)
    mask = os.urandom(4)
    masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
    sock.sendall(header + mask + masked)


def read_frame(sock: TunnelSocket) -> tuple[int, bytes]:
    first, second = recv_exact(sock, 2)
    opcode = first & 0x0F
    length = second & 0x7F
    if length == 126:
        length = struct.unpack("!H", recv_exact(sock, 2))[0]
    elif length == 127:
        length = struct.unpack("!Q", recv_exact(sock, 8))[0]
    mask = recv_exact(sock, 4) if second & 0x80 else None
    payload = recv_exact(sock, length)
    if mask:
        payload = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
    return opcode, payload


def connect(host: str, port: int) -> TunnelSocket:
    raw = socket.create_connection((host, port), timeout=30)
    sock = ssl.create_default_context().wrap_socket(raw, server_hostname=host)
    key = base64.b64encode(os.urandom(16)).decode("ascii")
    request = (
        "GET / HTTP/1.1\r\n"
        f"Host: {host}\r\n"
        "Connection: Upgrade\r\n"
        "Upgrade: websocket\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n\r\n"
    )
    sock.sendall(request.encode("ascii"))
    response = bytearray()
    while b"\r\n\r\n" not in response:
        chunk = sock.recv(4096)
        if not chunk:
            raise RuntimeError("WebSocket server closed during upgrade")
        response.extend(chunk)
        if len(response) > 65536:
            raise RuntimeError("WebSocket upgrade headers are too large")
    headers, remainder = bytes(response).split(b"\r\n\r\n", 1)
    status = headers.split(b"\r\n", 1)[0]
    if b" 101 " not in status:
        raise RuntimeError(f"WebSocket upgrade failed: {status.decode(errors='replace')}")
    return TunnelSocket(sock, remainder)


def pump_remote(sock: TunnelSocket, stopped: threading.Event) -> None:
    stdout = sys.stdout.buffer
    try:
        while not stopped.is_set():
            opcode, payload = read_frame(sock)
            if opcode in (0x0, 0x1, 0x2):
                stdout.write(payload)
                stdout.flush()
            elif opcode == 0x8:
                break
            elif opcode == 0x9:
                send_frame(sock, payload, opcode=0xA)
    except (EOFError, OSError) as error:
        print(f"WebSocket SSH proxy receive error: {error}", file=sys.stderr)
    finally:
        stopped.set()


def pump_stdin(sock: TunnelSocket, stopped: threading.Event) -> None:
    stdin = sys.stdin.buffer
    try:
        while not stopped.is_set():
            payload = os.read(stdin.fileno(), 65536)
            if not payload:
                break
            send_frame(sock, payload)
    except OSError as error:
        print(f"WebSocket SSH proxy send error: {error}", file=sys.stderr)
    finally:
        stopped.set()
        try:
            send_frame(sock, b"", opcode=0x8)
        except OSError:
            pass


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} HOST PORT", file=sys.stderr)
        return 2
    sock = connect(sys.argv[1], int(sys.argv[2]))
    stopped = threading.Event()
    sender = threading.Thread(target=pump_stdin, args=(sock, stopped), daemon=True)
    sender.start()
    # Keep the remote reader on the main thread. When SSH closes the tunnel we
    # can exit immediately without waiting on a blocking Windows stdin read.
    pump_remote(sock, stopped)
    sock.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
