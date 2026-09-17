"""A stand-in for `arbos-hub`'s `/attach/<machine>/<project>` route: a WebSocket that checks the
client token and relays attach frames to a mock kernel's TCP socket, one JSON frame per text
message kernel-to-client, newline-delimited lines client-to-kernel — the shape the real hub uses.
An unknown name answers with the hub's `error` frame and closes.
"""

from __future__ import annotations

import asyncio
import json
import logging
from http import HTTPStatus
from urllib.parse import parse_qs, urlsplit

from websockets.asyncio.server import ServerConnection, serve

log = logging.getLogger("mock.hub")


class MockHub:
    def __init__(self, token: str, host: str = "127.0.0.1", port: int = 0):
        self.token = token
        self.host, self.port = host, port
        self.kernels: dict[str, str] = {}  # "machine/project" -> tcp://host:port
        self.places: dict[str, str] = {}  # "machine/project" -> the folder that kernel serves
        self.attaches: list[str] = []
        self._server = None

    async def start(self) -> str:
        self._server = await serve(self._handler, self.host, self.port, process_request=self._auth, compression=None)
        self.port = self._server.sockets[0].getsockname()[1]
        return f"ws://{self.host}:{self.port}"

    async def stop(self) -> None:
        if self._server:
            self._server.close()
            await self._server.wait_closed()

    def _auth(self, connection: ServerConnection, request):
        parts = urlsplit(request.path)
        if parts.path == "/list":
            # The roster names each kernel's folder (`place`), as the real hub does: that is how the
            # gateway learns the call's working directory.
            body = json.dumps({"machines": [
                {"name": k.split("/")[0], "projects": [{"name": k.split("/")[1], "live": True, "place": self.places.get(k)}]}
                for k in self.kernels
            ]})
            return connection.respond(HTTPStatus.OK, body)
        query = parse_qs(parts.query).get("token", [""])[0]
        header = request.headers.get("Authorization", "")
        bearer = header[7:] if header.lower().startswith("bearer ") else ""
        if query != self.token and bearer != self.token:
            return connection.respond(HTTPStatus.UNAUTHORIZED, "unauthorized\n")
        return None

    async def _handler(self, ws: ServerConnection) -> None:
        path = urlsplit(ws.request.path).path
        name = path.removeprefix("/attach/").strip("/")
        self.attaches.append(name)
        url = self.kernels.get(name)
        if not url:
            await ws.send(json.dumps({"type": "error", "detail": f"hub: no kernel named {name!r}"}))
            await ws.close()
            return
        host, port = url.removeprefix("tcp://").rsplit(":", 1)
        reader, writer = await asyncio.open_connection(host, int(port))

        async def to_client() -> None:
            try:
                while True:
                    line = await reader.readline()
                    if not line:
                        break
                    await ws.send(line.decode().rstrip("\n"))
            except Exception:
                pass

        pump = asyncio.create_task(to_client())
        try:
            async for message in ws:
                text = message if isinstance(message, str) else bytes(message).decode()
                for line in text.splitlines():
                    if line.strip():
                        writer.write((line + "\n").encode())
                await writer.drain()
        except Exception:
            pass
        finally:
            pump.cancel()
            writer.close()
