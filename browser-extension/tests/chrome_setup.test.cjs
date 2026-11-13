const { test } = require("node:test");
const assert = require("node:assert/strict");
const { webcrypto } = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

function options(stored = {}) {
  const nodes = new Map();
  const messages = [];
  const node = key => {
    if (!nodes.has(key)) {
      nodes.set(key, {
        checked: false,
        parentElement: {},
        value: "",
        listeners: {},
        addEventListener(event, listener) { this.listeners[event] = listener; },
      });
    }
    return nodes.get(key);
  };
  const document = {
    documentElement: {},
    getElementById: node,
    querySelector: node,
    querySelectorAll: () => [],
  };
  const chrome = {
    i18n: { getMessage: key => key, getUILanguage: () => "fa" },
    runtime: {
      id: "a".repeat(32),
      sendNativeMessage: async (host, message) => {
        messages.push({ host, message });
        return { version: 1, requestId: message.requestId, status: "ready" };
      },
    },
    storage: {
      local: {
        get: async () => ({ ...stored }),
        set: async patch => Object.assign(stored, patch),
        remove: async keys => keys.forEach(key => { delete stored[key]; }),
      },
    },
    action: { setBadgeText: async () => {} },
  };
  const context = vm.createContext({ chrome, document, crypto: webcrypto, TextEncoder });
  const source = name => fs.readFileSync(path.resolve(__dirname, "../shared", name), "utf8");
  vm.runInContext(source("protocol.js").replace("com.dlm.browser", "com.dlm.chrome"), context);
  vm.runInContext(source("options.js"), context);
  return { document, messages, node, stored };
}

const flush = () => new Promise(resolve => setImmediate(resolve));

test("managed options wait for browser consent and then ping automatically on save", async () => {
  const page = options();
  await flush();
  assert.equal(page.messages.length, 0);
  assert.equal(page.document.documentElement.dir, "rtl");
  assert.equal(page.node("[data-i18n='save']").textContent, "allowAndConnect");
  const submit = () => page.node("pairing-form").listeners.submit({ preventDefault() {} });
  await submit();
  assert.equal(page.messages.length, 0);
  page.node("pairing-code").value = "a".repeat(64);
  assert.equal(page.stored.shareSelectedLinks, undefined);
  assert.equal(page.messages.length, 0);
  await submit();
  assert.equal(page.stored.shareSelectedLinks, true);
  assert.equal(page.messages.length, 1);
  assert.equal(page.messages[0].host, "com.dlm.chrome");
  assert.equal(page.messages[0].message.operation, "ping");
  assert.equal(page.messages[0].message.url, undefined);
  assert.equal(page.node("status").textContent, "ready");
});

test("opening managed options retests saved pairing only with saved consent", async () => {
  const saved = { pairingToken: "b".repeat(64), shareSelectedLinks: true };
  const paired = options(saved);
  const unpaired = options({ ...saved, shareSelectedLinks: false });
  await flush();
  assert.equal(paired.messages.length, 1);
  assert.equal(unpaired.messages.length, 0);
});

test("forget pairing removes consent and blocks the next managed ping", async () => {
  const page = options({ pairingToken: "b".repeat(64), shareSelectedLinks: true });
  await flush();
  assert.equal(page.messages.length, 1);
  await page.node("forget").listeners.click();
  await page.node("test").listeners.click();
  assert.equal(page.messages.length, 1);
  assert.equal(page.stored.pairingToken, undefined);
  assert.equal(page.stored.shareSelectedLinks, undefined);
});
