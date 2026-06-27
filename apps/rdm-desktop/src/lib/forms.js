import { isHttpUrl, isValidWindowsFilename, normalizeSha256 } from "./validate.js";

export function validateDownloadForm(values) {
  const url = values.url.trim();
  const destination = values.destination.trim();
  const filename = values.filename.trim();

  if (!isHttpUrl(url)) {
    return { error: "请输入有效的 HTTP 或 HTTPS 地址。" };
  }
  if (!destination) {
    return { error: "请选择保存目录。" };
  }
  if (filename && !isValidWindowsFilename(filename)) {
    return { error: "文件名不符合 Windows 命名规则。" };
  }

  const checksum = normalizeSha256(values.sha256);
  if (checksum.error) return checksum;

  return {
    value: {
      url,
      destination,
      connections: Number(values.connections),
      filename,
      sha256: checksum.value ?? "",
    },
  };
}

export function validateSettingsForm(values) {
  const downloadDir = values.downloadDir.trim();
  const proxyUrl = values.proxyUrl.trim();

  if (!downloadDir) {
    return { error: "请选择默认下载目录。" };
  }
  if (values.proxyEnabled && !proxyUrl) {
    return { error: "已启用代理，请填写代理地址。" };
  }
  if (proxyUrl && !/^https?:\/\//i.test(proxyUrl) && !/^socks5h?:\/\//i.test(proxyUrl)) {
    return { error: "代理地址需以 http://、https:// 或 socks5:// 开头。" };
  }

  return {
    value: {
      download_dir: downloadDir,
      max_active_downloads: Number(values.maxActive),
      default_connections: Number(values.connections),
      retry_count: Number(values.retry),
      speed_limit_bytes: Number(values.speedKb) * 1024,
      clipboard_monitoring: values.clipboard,
      minimize_to_tray: values.tray,
      hls_transcode: values.hlsTranscode,
      theme: values.theme,
      proxy_enabled: values.proxyEnabled,
      proxy_url: proxyUrl,
      proxy_username: values.proxyUsername.trim(),
      proxy_password: values.proxyPassword,
    },
  };
}
