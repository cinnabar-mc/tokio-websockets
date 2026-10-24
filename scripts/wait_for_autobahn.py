"""Wait at most 30 seconds for the example's Autobahn WebSocket endpoint."""

import base64
import hashlib
import os
import socket
import time

host, port = "localhost", 9001
key = base64.b64encode(os.urandom(16))
accept = base64.b64encode(
    hashlib.sha1(key + b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest()
)
request = (
    b"GET /getCaseCount HTTP/1.1\r\n"
    + f"Host: {host}:{port}\r\n".encode()
    + b"Upgrade: websocket\r\nConnection: Upgrade\r\n"
    + b"Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: "
    + key
    + b"\r\n\r\n"
)
deadline = time.monotonic() + 30
last_error = "no response"
while time.monotonic() < deadline:
    try:
        attempt_deadline = min(deadline, time.monotonic() + 1)
        # Match the example's default Gai resolver: use its first localhost address.
        family, kind, protocol, _, address = socket.getaddrinfo(
            host, port, type=socket.SOCK_STREAM
        )[0]
        with socket.socket(family, kind, protocol) as connection:
            connection.settimeout(max(0.001, attempt_deadline - time.monotonic()))
            connection.connect(address)
            connection.settimeout(max(0.001, attempt_deadline - time.monotonic()))
            connection.sendall(request)
            response = b""
            while b"\r\n\r\n" not in response:
                remaining = attempt_deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("WebSocket upgrade timed out")
                connection.settimeout(remaining)
                chunk = connection.recv(4096)
                if not chunk:
                    raise ConnectionError("server closed before WebSocket upgrade")
                response += chunk
                if len(response) > 16384:
                    raise ValueError("WebSocket upgrade headers exceed 16 KiB")
            lines = response.split(b"\r\n\r\n", 1)[0].split(b"\r\n")
            headers = dict(line.split(b":", 1) for line in lines[1:])
            headers = {name.lower(): value.strip() for name, value in headers.items()}
            if (
                lines[0].split()[:2] != [b"HTTP/1.1", b"101"]
                or headers.get(b"upgrade", b"").lower() != b"websocket"
                or headers.get(b"sec-websocket-accept") != accept
            ):
                raise ValueError("server has not accepted a WebSocket upgrade")
            print("Autobahn WebSocket server is ready")
            break
    except (OSError, ValueError) as error:
        last_error = str(error)
    time.sleep(min(0.1, max(0, deadline - time.monotonic())))
else:
    raise SystemExit(f"Autobahn WebSocket server did not become ready: {last_error}")
