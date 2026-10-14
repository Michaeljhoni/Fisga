const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);
const t = (key) => api.i18n.getMessage(key);

for (const el of document.querySelectorAll("[data-i18n]")) {
  el.textContent = t(el.dataset.i18n) || el.textContent;
}

async function init() {
  const prefs = await api.storage.local.get({ overlay: true, intercept: true });
  $("overlay").checked = prefs.overlay !== false;
  $("intercept").checked = prefs.intercept !== false;
  $("overlay").addEventListener("change", (e) => api.storage.local.set({ overlay: e.target.checked }));
  $("intercept").addEventListener("change", (e) => api.storage.local.set({ intercept: e.target.checked }));

  // Firefox treats host permissions as optional: without them no button and no sniffing.
  const all = { origins: ["<all_urls>"] };
  if (api.permissions?.contains && !(await api.permissions.contains(all))) {
    $("grant").hidden = false;
    $("grant-button").addEventListener("click", async () => {
      if (await api.permissions.request(all)) $("grant").hidden = true;
    });
  }

  const status = await api.runtime.sendMessage({ type: "ping" });
  $("status").textContent = status?.ok ? `${t("popupConnected")} · v${status.version}` : t("popupDisconnected");
  $("status").className = `status ${status?.ok ? "ok" : "err"}`;
  $("hint").hidden = Boolean(status?.ok);

  const [tab] = await api.tabs.query({ active: true, currentWindow: true });
  if (!tab || !/^https?:/.test(tab.url ?? "")) {
    $("empty").hidden = false;
    return;
  }
  const res = await api.runtime.sendMessage({ type: "popup-candidates", tabId: tab.id });
  const items = res?.items ?? [];
  $("empty").hidden = items.length > 1;
  $("media").replaceChildren(
    ...items.map((item) => {
      const li = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.title = item.url;
      button.textContent = item.label;
      const small = document.createElement("small");
      small.textContent = item.detail;
      button.append(small);
      button.addEventListener("click", async () => {
        const reply = await api.runtime.sendMessage({ type: "popup-download", tabId: tab.id, item });
        button.classList.add(reply?.ok ? "sent" : "failed");
        small.textContent = reply?.ok ? `✓ ${t("sent")}` : reply?.error ?? t("failed");
      });
      li.append(button);
      return li;
    }),
  );
}

init();
