import { createHash } from "node:crypto";
import http from "node:http";

const host = process.env.RDM_SMOKE_HOST || "127.0.0.1";
const port = Number(process.env.RDM_SMOKE_PORT || 18080);

function fixture(size) {
  const value = Buffer.allocUnsafe(size);
  for (let index = 0; index < size; index += 1) {
    value[index] = (index * 31 + 17) % 251;
  }
  return value;
}

const fixtures = {
  "/range.bin": fixture(16 * 1024 * 1024),
  "/slow.bin": fixture(64 * 1024 * 1024),
  "/no-range.bin": fixture(4 * 1024 * 1024),
  "/empty.bin": Buffer.alloc(0),
};

function checksum(value) {
  return createHash("sha256").update(value).digest("hex");
}

function commonHeaders(path, body) {
  return {
    "Content-Type": "application/octet-stream",
    "Content-Disposition": `attachment; filename="${path.slice(1)}"`,
    "Content-Length": body.length,
    "Cache-Control": "no-store",
  };
}

function parseRange(header, size) {
  const match = /^bytes=(\d+)-(\d*)$/.exec(header || "");
  if (!match) return null;

  const start = Number(match[1]);
  const end = match[2] ? Number(match[2]) : size - 1;
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start > end || start >= size) {
    return { invalid: true };
  }
  return { start, end: Math.min(end, size - 1) };
}

function sendBuffer(request, response, path, body, supportsRange) {
  const range = supportsRange ? parseRange(request.headers.range, body.length) : null;
  if (range?.invalid) {
    response.writeHead(416, {
      "Content-Range": `bytes */${body.length}`,
      "Content-Length": 0,
    });
    response.end();
    return;
  }

  const selected = range ? body.subarray(range.start, range.end + 1) : body;
  const headers = commonHeaders(path, selected);
  if (supportsRange) headers["Accept-Ranges"] = "bytes";
  if (range) headers["Content-Range"] = `bytes ${range.start}-${range.end}/${body.length}`;

  response.writeHead(range ? 206 : 200, headers);
  if (request.method === "HEAD") response.end();
  else response.end(selected);
}

function sendSlow(request, response, body) {
  const range = parseRange(request.headers.range, body.length);
  if (range?.invalid) {
    response.writeHead(416, {
      "Content-Range": `bytes */${body.length}`,
      "Content-Length": 0,
    });
    response.end();
    return;
  }

  const start = range ? range.start : 0;
  const end = range ? range.end : body.length - 1;
  const selected = body.subarray(start, end + 1);
  const headers = {
    ...commonHeaders("/slow.bin", selected),
    "Accept-Ranges": "bytes",
  };
  if (range) headers["Content-Range"] = `bytes ${start}-${end}/${body.length}`;
  response.writeHead(range ? 206 : 200, headers);
  if (request.method === "HEAD") {
    response.end();
    return;
  }

  let offset = 0;
  const chunkSize = 64 * 1024;
  const timer = setInterval(() => {
    if (response.destroyed || offset >= selected.length) {
      clearInterval(timer);
      if (!response.destroyed) response.end();
      return;
    }
    const next = Math.min(offset + chunkSize, selected.length);
    response.write(selected.subarray(offset, next));
    offset = next;
  }, 25);
  response.on("close", () => clearInterval(timer));
}

const server = http.createServer((request, response) => {
  const url = new URL(request.url || "/", `http://${host}:${port}`);
  if (!["GET", "HEAD"].includes(request.method || "")) {
    response.writeHead(405, { Allow: "GET, HEAD", "Content-Length": 0 });
    response.end();
    return;
  }

  if (url.pathname === "/redirect.bin") {
    response.writeHead(302, { Location: "/range.bin", "Content-Length": 0 });
    response.end();
    return;
  }
  if (url.pathname === "/error.bin") {
    response.writeHead(500, { "Content-Type": "text/plain", "Content-Length": 19 });
    response.end("intentional failure\n");
    return;
  }
  if (url.pathname === "/slow.bin") {
    sendSlow(request, response, fixtures[url.pathname]);
    return;
  }
  if (url.pathname in fixtures) {
    sendBuffer(
      request,
      response,
      url.pathname,
      fixtures[url.pathname],
      url.pathname !== "/no-range.bin",
    );
    return;
  }

  const routes = Object.keys(fixtures).concat("/redirect.bin", "/error.bin").join("\n");
  const body = `RDM smoke server\n\n${routes}\n`;
  response.writeHead(url.pathname === "/" ? 200 : 404, {
    "Content-Type": "text/plain; charset=utf-8",
    "Content-Length": Buffer.byteLength(body),
  });
  response.end(request.method === "HEAD" ? undefined : body);
});

server.listen(port, host, () => {
  console.log(`RDM smoke server: http://${host}:${port}`);
  for (const [path, body] of Object.entries(fixtures)) {
    console.log(`${path.padEnd(16)} ${body.length.toString().padStart(9)} bytes  sha256=${checksum(body)}`);
  }
  console.log("Press Ctrl+C to stop.");
});

server.on("error", (error) => {
  console.error(`Smoke server failed: ${error.message}`);
  process.exitCode = 1;
});
