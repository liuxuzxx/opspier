"""Loopback-only SSH protocol fixture. Generated host keys never leave memory."""
import base64
import hashlib
import json
import logging
import os
import socket
import sys
import threading
import time

import paramiko

logging.getLogger("paramiko").addHandler(logging.NullHandler())
logging.getLogger("paramiko").propagate = False
expected_password='fixture-only'
if '--password-stdin' in sys.argv:
    expected_password=json.loads(sys.stdin.readline())['password']
host_key = paramiko.RSAKey.generate(2048)
listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen(16)


def trace(event, **values):
    path = os.environ.get("AIC_OPS_FIXTURE_TRACE")
    if path:
        with open(path, "a") as output:
            output.write(json.dumps({"event": event, "time": time.monotonic(), **values}) + "\n")


def public_info():
    fingerprint = base64.b64encode(hashlib.sha256(host_key.asbytes()).digest()).decode().rstrip("=")
    return {"port": listener.getsockname()[1], "fingerprint": "SHA256:" + fingerprint}


class Server(paramiko.ServerInterface):
    def __init__(self):
        self.ready = threading.Event()
        self.size = (0, 0)
        self.connection_id = threading.get_ident()

    def get_allowed_auths(self, username):
        return "password,publickey"

    def check_auth_password(self, username, password):
        accepted = username == "fixture-user" and password == expected_password
        trace("authentication", accepted=accepted)
        return paramiko.AUTH_SUCCESSFUL if accepted else paramiko.AUTH_FAILED

    def check_auth_publickey(self, username, key):
        return paramiko.AUTH_SUCCESSFUL if username == "fixture-user" and key.get_bits() >= 2048 else paramiko.AUTH_FAILED

    def check_channel_request(self, kind, channel_id):
        trace("channel")
        return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def check_channel_pty_request(self, channel, term, width, height, pixelwidth, pixelheight, modes):
        trace("pty")
        self.size = (width, height)
        return term == b"xterm-256color"

    def check_channel_window_change_request(self, channel, width, height, pixelwidth, pixelheight):
        self.size = (width, height)
        trace("resize", connection=self.connection_id, width=width, height=height)
        return True

    def check_channel_shell_request(self, channel):
        trace("shell")
        self.ready.set()
        return True


def connection(stream, key):
    transport = paramiko.Transport(stream)
    try:
        transport.add_server_key(key)
        server = Server()
        transport.start_server(server=server)
        channel = transport.accept(10)
        if channel is None or not server.ready.wait(10):
            return
        channel.sendall("READY 中文😀\r\n".encode())
        pending = b""
        while True:
            part = channel.recv(65536)
            if not part:
                return
            # Emulate a canonical PTY: Enter/paste can arrive as CR or CRLF.
            pending += part.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
            while b"\n" in pending:
                line, pending = pending.split(b"\n", 1)
                if line == b"exit":
                    channel.send_exit_status(0)
                    channel.close()
                    return
                if line == b"size":
                    channel.sendall(("SIZE %d %d\r\n" % server.size).encode())
                elif line == b"flood":
                    trace("flood-start")
                    for _ in range(128):
                        channel.sendall(b"x" * 8192)
                    channel.sendall_stderr(b"STDERR-END\r\n")
                    channel.sendall(b"FLOOD-END\r\n")
                    trace("flood-end", bytes=1024 * 1024)
                else:
                    trace("echo", connection=server.connection_id, bytes=len(line), digest=hashlib.sha256(line).hexdigest())
                    channel.sendall(line + b"\r\n")
    except (EOFError, OSError, paramiko.SSHException):
        pass
    finally:
        transport.close()


def accept():
    while True:
        stream, _ = listener.accept()
        threading.Thread(target=connection, args=(stream, host_key), daemon=True).start()


threading.Thread(target=accept, daemon=True).start()
print(json.dumps(public_info()), flush=True)
for line in sys.stdin:
    if line.strip() == "rotate":
        host_key = paramiko.RSAKey.generate(2048)
        print(json.dumps(public_info()), flush=True)
