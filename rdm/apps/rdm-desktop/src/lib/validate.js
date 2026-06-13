// Input validation mirroring rdm-domain/validation.rs (and the Python rules).

export function isHttpUrl(value) {
  try {
    const parsed = new URL(value.trim());
    return (
      (parsed.protocol === "http:" || parsed.protocol === "https:") &&
      parsed.host.length > 0
    );
  } catch {
    return false;
  }
}

const INVALID_CHARS = /[<>:"/\\|?*\x00-\x1f]/;
const RESERVED = new Set([
  "CON",
  "PRN",
  "AUX",
  "NUL",
  ...Array.from({ length: 9 }, (_, i) => `COM${i + 1}`),
  ...Array.from({ length: 9 }, (_, i) => `LPT${i + 1}`),
]);

export function isValidWindowsFilename(value) {
  if (!value || value !== value.replace(/[. ]+$/, "")) return false;
  if (INVALID_CHARS.test(value)) return false;
  const stem = value.split(".")[0].toUpperCase();
  return !RESERVED.has(stem);
}

// Returns { value } (string|null) on success, or { error } on failure.
export function normalizeSha256(value) {
  const checksum = value.trim().toLowerCase();
  if (!checksum) return { value: null };
  if (checksum.length !== 64 || !/^[0-9a-f]{64}$/.test(checksum)) {
    return { error: "SHA-256 必须是 64 位十六进制字符。" };
  }
  return { value: checksum };
}
