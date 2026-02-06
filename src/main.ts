import { invoke } from "@tauri-apps/api/core";

type ProxyConfig = {
  aggregated?: string | null;
  http?: string | null;
  https?: string | null;
  socks5?: string | null;
};

type ProxySaveResult = {
  available: boolean;
  last_error?: string | null;
};

function qs<T extends HTMLElement>(selector: string): T {
  const el = document.querySelector(selector);
  if (!el) throw new Error(`Missing element: ${selector}`);
  return el as T;
}

function getView(): string {
  const url = new URL(window.location.href);
  return url.searchParams.get("view") ?? "";
}

function inputRow(label: string, id: string, placeholder: string) {
  const row = document.createElement("div");
  row.className = "tokbar-row";

  const left = document.createElement("label");
  left.className = "tokbar-label";
  left.htmlFor = id;
  left.textContent = label;

  const input = document.createElement("input");
  input.className = "tokbar-input";
  input.id = id;
  input.placeholder = placeholder;
  input.autocomplete = "off";
  input.spellcheck = false;

  row.append(left, input);
  return { row, input };
}

function normalizeOptionalText(value: string): string | null {
  const trimmed = value.trim();
  return trimmed.length ? trimmed : null;
}

async function renderProxySettings(root: HTMLElement) {
  root.innerHTML = "";

  const wrap = document.createElement("div");
  wrap.className = "tokbar-wrap";

  const title = document.createElement("div");
  title.className = "tokbar-title";
  title.textContent = "Proxy 设置";

  const desc = document.createElement("div");
  desc.className = "tokbar-desc";
  desc.textContent =
    "用于获取 LiteLLM 模型价格（GitHub RAW）。聚合代理优先；为空则使用分开代理。支持：127.0.0.1:7897 或带协议（http:// / socks5://）。";

  const aggregated = inputRow("聚合代理", "proxy-aggregated", "127.0.0.1:7897");
  const http = inputRow("HTTP", "proxy-http", "127.0.0.1:7897");
  const https = inputRow("HTTPS", "proxy-https", "127.0.0.1:7897");
  const socks5 = inputRow("SOCKS5", "proxy-socks5", "127.0.0.1:7897");

  const buttonRow = document.createElement("div");
  buttonRow.className = "tokbar-actions";

  const status = document.createElement("div");
  status.className = "tokbar-status";

  const save = document.createElement("button");
  save.className = "tokbar-button";
  save.textContent = "确认并重试获取价格";

  const clear = document.createElement("button");
  clear.className = "tokbar-button tokbar-button-secondary";
  clear.textContent = "清空";

  buttonRow.append(save, clear);

  wrap.append(
    title,
    desc,
    aggregated.row,
    http.row,
    https.row,
    socks5.row,
    buttonRow,
    status,
  );
  root.append(wrap);

  const existing = (await invoke("tokbar_get_proxy_config")) as ProxyConfig;
  aggregated.input.value = existing.aggregated ?? "";
  http.input.value = existing.http ?? "";
  https.input.value = existing.https ?? "";
  socks5.input.value = existing.socks5 ?? "";

  function setStatus(text: string, kind: "ok" | "err" | "info") {
    status.textContent = text;
    status.dataset.kind = kind;
  }

  clear.addEventListener("click", () => {
    aggregated.input.value = "";
    http.input.value = "";
    https.input.value = "";
    socks5.input.value = "";
    setStatus("已清空（还未保存）", "info");
  });

  save.addEventListener("click", async () => {
    save.disabled = true;
    clear.disabled = true;
    try {
      setStatus("保存中…", "info");
      const cfg: ProxyConfig = {
        aggregated: normalizeOptionalText(aggregated.input.value),
        http: normalizeOptionalText(http.input.value),
        https: normalizeOptionalText(https.input.value),
        socks5: normalizeOptionalText(socks5.input.value),
      };

      const result = (await invoke("tokbar_set_proxy_config", {
        config: cfg,
      })) as ProxySaveResult;

      if (result.available) {
        setStatus("已连接：模型价格获取成功。", "ok");
      } else {
        const reason = (result.last_error ?? "").trim();
        setStatus(
          reason.length
            ? `仍无法获取模型价格：${reason}`
            : "仍无法获取模型价格：请检查代理是否可用。",
          "err",
        );
      }
    } catch (e) {
      setStatus(`保存失败：${String(e)}`, "err");
    } finally {
      save.disabled = false;
      clear.disabled = false;
    }
  });
}

function renderEmpty(root: HTMLElement) {
  root.innerHTML = "";
}

window.addEventListener("DOMContentLoaded", async () => {
  const root = qs<HTMLElement>("#app");
  const view = getView();
  if (view === "proxy") {
    await renderProxySettings(root);
  } else {
    renderEmpty(root);
  }
});

