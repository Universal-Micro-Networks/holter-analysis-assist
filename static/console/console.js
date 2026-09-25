(function () {
  "use strict";

  var HEALTH_PATH = "/health";
  var ANALYZE_PATH = "/v1/analyze";

  var healthBtn = document.getElementById("health-btn");
  var healthStatus = document.getElementById("health-status");
  var eclInput = document.getElementById("ecl-file");
  var formatSelect = document.getElementById("format-select");
  var analyzeBtn = document.getElementById("analyze-btn");
  var loadingEl = document.getElementById("loading");
  var errorEl = document.getElementById("error");
  var resultEl = document.getElementById("result");
  var downloadBtn = document.getElementById("download-btn");

  var lastResult = null;
  var busy = false;

  function setBusy(isBusy) {
    busy = isBusy;
    loadingEl.hidden = !isBusy;
    healthBtn.disabled = isBusy;
    analyzeBtn.disabled = isBusy;
    eclInput.disabled = isBusy;
    formatSelect.disabled = isBusy;
  }

  function clearError() {
    errorEl.textContent = "";
  }

  function showError(message) {
    errorEl.textContent = message;
  }

  function clearResult() {
    lastResult = null;
    resultEl.textContent = "";
    downloadBtn.disabled = true;
  }

  function showResult(body, kind, filename) {
    lastResult = { body: body, kind: kind, filename: filename };
    resultEl.textContent = body;
    downloadBtn.disabled = false;
  }

  function prettyMaybeJson(text) {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch (e) {
      return text;
    }
  }

  function extractUpstreamError(text) {
    try {
      var data = JSON.parse(text);
      if (data && data.error) {
        var code = data.error.code || "";
        var msg = data.error.message || "";
        if (code && msg) {
          return code + ": " + msg;
        }
        return code || msg || text;
      }
    } catch (e) {
      /* not JSON */
    }
    return text;
  }

  function guessKind(contentType, body) {
    var ct = (contentType || "").toLowerCase();
    if (ct.indexOf("json") !== -1) {
      return "json";
    }
    if (ct.indexOf("csv") !== -1) {
      return "csv";
    }
    var trimmed = body.replace(/^\uFEFF/, "").trim();
    if (trimmed.charAt(0) === "{" || trimmed.charAt(0) === "[") {
      return "json";
    }
    return "csv";
  }

  function downloadName(kind) {
    if (kind === "json") {
      return "analyze-result.json";
    }
    if (kind === "csv") {
      return "analyze-result.csv";
    }
    return "analyze-result.txt";
  }

  function mimeFor(kind) {
    if (kind === "json") {
      return "application/json";
    }
    if (kind === "csv") {
      return "text/csv";
    }
    return "text/plain";
  }

  async function onHealth() {
    clearError();
    healthStatus.className = "console__status";
    healthStatus.textContent = "確認中…";
    setBusy(true);
    try {
      var res = await fetch(HEALTH_PATH, { method: "GET" });
      if (!res.ok) {
        healthStatus.className = "console__status is-error";
        healthStatus.textContent = "ヘルス確認に失敗しました（HTTP " + res.status + "）";
        showError("ヘルス確認に失敗しました。サービスへ到達できないか、非成功応答が返されました。");
        return;
      }
      var text = await res.text();
      var ok = false;
      try {
        var data = JSON.parse(text);
        ok = data && data.status === "ok";
      } catch (e) {
        ok = false;
      }
      if (ok) {
        healthStatus.className = "console__status is-ok";
        healthStatus.textContent = "正常（status: ok）";
      } else {
        healthStatus.className = "console__status is-error";
        healthStatus.textContent = "ヘルス確認に失敗しました（応答形式が想定外です）";
        showError("ヘルス確認に失敗しました。応答を解釈できませんでした。");
      }
    } catch (err) {
      healthStatus.className = "console__status is-error";
      healthStatus.textContent = "ヘルス確認に失敗しました";
      showError("ヘルス確認に失敗しました。サーバーに到達できません。");
    } finally {
      setBusy(false);
    }
  }

  async function onAnalyze() {
    clearError();
    clearResult();

    var file = eclInput.files && eclInput.files[0];
    if (!file) {
      showError("ECL ファイルが選択されていません。解析の前にファイルを選択してください。");
      return;
    }

    var form = new FormData();
    form.append("ecl", file, file.name);
    var format = formatSelect.value;
    if (format) {
      form.append("format", format);
    }

    setBusy(true);
    try {
      var res = await fetch(ANALYZE_PATH, { method: "POST", body: form });
      var text = await res.text();
      if (!res.ok) {
        var detail = extractUpstreamError(text);
        var base = "解析に失敗しました（HTTP " + res.status + "）。";
        if (res.status === 504) {
          base = "解析に失敗しました。リクエストがタイムアウトしました。";
        }
        showError(detail ? base + " " + detail : base);
        return;
      }
      var kind = guessKind(res.headers.get("content-type"), text);
      var display = kind === "json" ? prettyMaybeJson(text) : text;
      showResult(display, kind, downloadName(kind));
    } catch (err) {
      showError("解析に失敗しました。サーバーに到達できないか、通信が中断されました。");
    } finally {
      setBusy(false);
    }
  }

  function onDownload() {
    if (!lastResult) {
      return;
    }
    var blob = new Blob([lastResult.body], { type: mimeFor(lastResult.kind) });
    var url = URL.createObjectURL(blob);
    var a = document.createElement("a");
    a.href = url;
    a.download = lastResult.filename || downloadName(lastResult.kind);
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }

  healthBtn.addEventListener("click", function () {
    if (busy) {
      return;
    }
    onHealth();
  });

  analyzeBtn.addEventListener("click", function () {
    if (busy) {
      return;
    }
    onAnalyze();
  });

  downloadBtn.addEventListener("click", onDownload);
})();
