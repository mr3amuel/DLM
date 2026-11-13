"use strict";
const api = globalThis.browser || chrome;
const codeInput = document.getElementById("pairing-code");
const consent = document.getElementById("consent");
const statusNode = document.getElementById("status");
const buttons = [...document.querySelectorAll("button")];
const i18n = key => api.i18n.getMessage(key);
const managed = ["com.dlm.chrome", "com.dlm.native_development.chrome"].includes(DLMProtocol.HOST);

document.documentElement.lang = api.i18n.getUILanguage().startsWith("fa") ? "fa" : "en";
document.documentElement.dir = document.documentElement.lang === "fa" ? "rtl" : "ltr";
document.querySelectorAll("[data-i18n]").forEach(node => {
  node.textContent = i18n(node.dataset.i18n);
});
document.getElementById("extension-id").textContent = api.runtime.id;

if (managed) {
  consent.required = false;
  consent.parentElement.hidden = true;
  document.getElementById("extension-id").parentElement.hidden = true;
  document.querySelector("[data-i18n='save']").textContent = i18n("allowAndConnect");
  document.querySelector("[data-i18n='installStep']").textContent = i18n("managedInstallStep");
  document.querySelector("[data-i18n='enableStep']").textContent = i18n("managedEnableStep");
  document.querySelector("[data-i18n='pairStep']").textContent = i18n("managedPairStep");
}

function show(status) {
  const keys = {
    ready: "ready", queued: "queued", already_queued: "queued",
    unpaired: "unpaired", disabled: "disabled", busy: "busy",
    outdated_app: "outdated", unsupported_version: "outdated",
    app_unavailable: "unavailable", host_unavailable: "hostMissing",
    unauthorized_extension: "unauthorized", invalid_url: "invalidUrl",
    unsafe_storage: "unsafeStorage", invalid_installation: "hostMissing",
    unsupported_platform: "unsupported",
  };
  statusNode.textContent = i18n(keys[status] || "unavailable");
}

let testing = false;
async function testConnection() {
  if (testing) return;
  testing = true;
  buttons.forEach(button => { button.disabled = true; });
  try {
    const {pairingToken, shareSelectedLinks} = await api.storage.local.get(
      ["pairingToken", "shareSelectedLinks"],
    );
    if (shareSelectedLinks !== true || (!managed && !consent.checked)) {
      show("unpaired");
      return;
    }
    const message = DLMProtocol.request("ping", pairingToken);
    const result = await api.runtime.sendNativeMessage(DLMProtocol.HOST, message);
    const status = DLMProtocol.response(result, message.requestId);
    await api.storage.local.set({lastStatus: status});
    show(status);
    if (status === "ready") await api.action.setBadgeText({text: ""});
  } catch (error) {
    show(error?.message === "unpaired" ? "unpaired" : "host_unavailable");
  } finally {
    buttons.forEach(button => { button.disabled = false; });
    testing = false;
  }
}

api.storage.local.get(["lastStatus", "shareSelectedLinks"]).then(
  ({lastStatus, shareSelectedLinks}) => {
    consent.checked = shareSelectedLinks === true;
    if (!managed && lastStatus) show(lastStatus);
    if (managed && consent.checked) void testConnection();
  },
);

document.getElementById("pairing-form").addEventListener("submit", async event => {
  event.preventDefault();
  if (testing) return;
  try {
    if (!managed && !consent.checked) throw new Error("unpaired");
    const code = codeInput.value.trim();
    DLMProtocol.request("ping", code);
    await api.storage.local.set({
      pairingToken: code,
      shareSelectedLinks: true,
      lastStatus: "unpaired",
    });
    consent.checked = true;
    codeInput.value = "";
    statusNode.textContent = i18n("saved");
    if (managed) await testConnection();
  } catch {
    show("unpaired");
  }
});

document.getElementById("test").addEventListener("click", testConnection);
document.getElementById("forget").addEventListener("click", async () => {
  await api.storage.local.remove(["pairingToken", "lastStatus", "shareSelectedLinks"]);
  consent.checked = false;
  codeInput.value = "";
  statusNode.textContent = i18n("forgotten");
});
