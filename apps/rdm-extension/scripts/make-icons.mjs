// 生成扩展用的占位 PNG 图标（纯色方块带圆角，无外部图像依赖）。
// 输出 icons/icon{16,48,128}.png。运行：npm run icons
//
// 仅使用 Node 内置 zlib，手写 PNG 编码，便于在没有图像工具链的 CI 上重生成。

import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const iconsDir = join(here, "..", "icons");
mkdirSync(iconsDir, { recursive: true });

// 品牌色，与桌面端 --accent 对齐。
const R = 0x5b;
const G = 0x74;
const B = 0xff;

const SIZES = [16, 48, 128];

// 一个简单的下载箭头图案（在 size×size 网格里定义），白色绘制在品牌底色上。
// 用相对单位（0..1）描述，按尺寸缩放，保证三个尺寸都清晰。
function drawArrow(size) {
  const buf = Buffer.alloc(size * size * 4); // RGBA
  const put = (x, y, [r, g, b, a]) => {
    if (x < 0 || y < 0 || x >= size || y >= size) return;
    const i = (y * size + x) * 4;
    // 简单 alpha 合成到不透明品牌底色上
    buf[i] = Math.round(r * (a / 255) + R * (1 - a / 255));
    buf[i + 1] = Math.round(g * (a / 255) + G * (1 - a / 255));
    buf[i + 2] = Math.round(b * (a / 255) + B * (1 - a / 255));
    buf[i + 3] = 255;
  };

  // 填充品牌底色
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      put(x, y, [R, G, B, 255]);
    }
  }

  const cx = (size - 1) / 2;
  // 箭头柄：竖线
  const stemHalfW = Math.max(1, size * 0.1);
  const stemTop = size * 0.24;
  const stemBottom = size * 0.58;
  // 箭头头：三角形
  const headTop = size * 0.5;
  const headBottom = size * 0.76;
  const headHalf = size * 0.3;

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const onStem =
        y >= stemTop && y <= stemBottom && Math.abs(x - cx) <= stemHalfW;
      // 三角形：在 headTop..headBottom 范围内，宽度随 y 线性增长
      let onHead = false;
      if (y >= headTop && y <= headBottom) {
        const t = (y - headTop) / (headBottom - headTop);
        const half = headHalf * t;
        if (Math.abs(x - cx) <= half) onHead = true;
      }
      if (onStem || onHead) put(x, y, [255, 255, 255, 255]);
    }
  }
  return buf;
}

// 编码一个 RGBA 缓冲区为 PNG（仅支持 8 位 truecolor + alpha）。
function encodePng(rgba, size) {
  // 每行前置一个过滤类型字节（0 = None）。
  const stride = size * 4;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0;
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride);
  }
  const idat = deflateSync(raw);

  const chunks = [];
  chunks.push(signature());
  chunks.push(chunk("IHDR", ihdr(size)));
  chunks.push(chunk("IDAT", idat));
  chunks.push(chunk("IEND", Buffer.alloc(0)));
  return Buffer.concat(chunks);
}

function signature() {
  return Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
}

function ihdr(size) {
  const b = Buffer.alloc(13);
  b.writeUInt32BE(size, 0); // width
  b.writeUInt32BE(size, 4); // height
  b[8] = 8; // bit depth
  b[9] = 6; // color type RGBA
  b[10] = 0; // compression
  b[11] = 0; // filter
  b[12] = 0; // interlace
  return b;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 0);
  return Buffer.concat([len, typeBuf, data, crc]);
}

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

for (const size of SIZES) {
  const rgba = drawArrow(size);
  const png = encodePng(rgba, size);
  const out = join(iconsDir, `icon${size}.png`);
  writeFileSync(out, png);
  console.log(`wrote ${out} (${png.length} bytes)`);
}
