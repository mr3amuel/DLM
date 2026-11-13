/* No DOM/page access. This module handles only the explicitly clicked link. */
"use strict";
globalThis.DLMProtocol = (() => {
  const VERSION = 1, HOST = "com.dlm.browser", MAX_BYTES = 16384;
  const statuses = new Set(["ready", "queued", "already_queued", "busy", "disabled", "unpaired", "unavailable", "app_unavailable", "invalid_request", "invalid_url", "invalid_filename", "invalid_frame", "unsupported_version", "outdated_app", "unauthorized_extension", "invalid_installation", "unsafe_storage", "storage_unavailable", "unsupported_platform", "random_unavailable"]);
  function token(value) { if (typeof value !== "string" || !/^[a-f0-9]{64}$/.test(value)) throw new Error("unpaired"); return value; }
  function request(operation, pairingToken, link, id = crypto.randomUUID()) {
    if (!["ping", "review"].includes(operation) || !/^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/i.test(id)) throw new Error("invalid_request");
    const message = {version: VERSION, requestId: id, operation, pairingToken: token(pairingToken)};
    if (operation === "review") {
      if (typeof link !== "string" || /[\s\u0000-\u001f\u007f]/u.test(link) || new TextEncoder().encode(link).length > 8192) throw new Error("invalid_url");
      let url; try {url = new URL(link);} catch {throw new Error("invalid_url");}
      if (!["http:", "https:"].includes(url.protocol) || !url.hostname || url.username || url.password) throw new Error("invalid_url");
      url.hash = ""; if(new TextEncoder().encode(url.href).length>8192)throw new Error("invalid_url");message.url = url.href;
      // Filename is suggested in DLM from this URL, not from page text or DOM.
    }
    if (new TextEncoder().encode(JSON.stringify(message)).length > MAX_BYTES) throw new Error("invalid_frame");
    return message;
  }
  function response(value, requestId) {
    if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).some(k => !["version","requestId","status"].includes(k)) || value.version !== VERSION || !statuses.has(value.status)) throw new Error("outdated_app");
    if (["ready","queued","already_queued"].includes(value.status) && value.requestId !== requestId) throw new Error("outdated_app");
    return value.status;
  }
  return Object.freeze({VERSION, HOST, request, response});
})();
