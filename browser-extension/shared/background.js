"use strict";
if (typeof importScripts === "function") importScripts("protocol.js");
const api = globalThis.browser || chrome;
const text = key => api.i18n.getMessage(key);
function installMenu() {
  const create = () => api.contextMenus.create({id:"dlm-review",title:text("downloadWithDlm"),contexts:["link"],targetUrlPatterns:["http://*/*","https://*/*"]});
  if (globalThis.browser) return api.contextMenus.removeAll().then(create);
  api.contextMenus.removeAll(() => {void api.runtime.lastError;create();});
}
api.runtime.onInstalled.addListener(() => {installMenu();api.runtime.openOptionsPage();});
api.runtime.onStartup.addListener(installMenu);
api.action.onClicked.addListener(() => api.runtime.openOptionsPage());
let sending = false;
api.contextMenus.onClicked.addListener(async info => {
  if (info.menuItemId !== "dlm-review" || sending) return;
  sending = true;
  try {
    const {pairingToken,shareSelectedLinks} = await api.storage.local.get(["pairingToken","shareSelectedLinks"]);
    if(shareSelectedLinks!==true)throw new Error("unpaired");
    const message = DLMProtocol.request("review", pairingToken, info.linkUrl);
    const result = await api.runtime.sendNativeMessage(DLMProtocol.HOST, message);
    const status = DLMProtocol.response(result, message.requestId);
    await api.storage.local.set({lastStatus:status});
    await api.action.setBadgeText({text:["queued","already_queued"].includes(status)?"":"!"});
    if (!["queued","already_queued"].includes(status)) await api.runtime.openOptionsPage();
  } catch (error) {
    // Never persist or show native exception text: it can contain sensitive inputs.
    const code = ["unpaired","invalid_url","invalid_request","invalid_frame","outdated_app"].includes(error?.message) ? error.message : "host_unavailable";
    await api.storage.local.set({lastStatus:code});
    await api.action.setBadgeText({text:"!"});
    await api.runtime.openOptionsPage();
  } finally {sending = false;}
});
