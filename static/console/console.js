(function () {
  "use strict";

  var HEALTH_PATH = "/health";
  var ANALYZE_PATH = "/v1/analyze";
  var HEALTH_POLL_MS = 10000;

  var healthStatus = document.getElementById("health-status");
  var healthLabel = document.getElementById("health-label");
  var eclInput = document.getElementById("ecl-file");
  var eclFilename = document.getElementById("ecl-filename");
  var formatSelect = document.getElementById("format-select");
  var analyzeBtn = document.getElementById("analyze-btn");
  var loadingEl = document.getElementById("loading");
  var errorEl = document.getElementById("error");
  var resultEl = document.getElementById("result");
  var downloadBtn = document.getElementById("download-btn");

  var lastResult = null;
  var busy = false;
  var healthInFlight = false;

  function setBusy(isBusy) {
    busy = isBusy;
    loadingEl.hidden = !isBusy;
    analyzeBtn.disabled = isBusy;
    eclInput.disabled = isBusy;
    formatSelect.disabled = isBusy;
    if (isBusy) {
      analyzeBtn.classList.add("is-loading");
    } else {
      analyzeBtn.classList.remove("is-loading");
    }
  }

  function clearError() {
    errorEl.textContent = "";
    errorEl.hidden = true;
  }

  function showError(message) {
    errorEl.textContent = message;
    errorEl.hidden = !message;
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

  function setHealthState(state, label) {
    healthStatus.className = "console-health";
    if (state === "ok") {
      healthStatus.classList.add("is-ok");
    } else if (state === "error") {
      healthStatus.classList.add("is-error");
    } else if (state === "pending") {
      healthStatus.classList.add("is-pending");
    }
    healthLabel.textContent = label;
  }

  /** Background poll — does not toggle analyze busy state. */
  async function pollHealth() {
    if (healthInFlight) {
      return;
    }
    healthInFlight = true;
    try {
      var res = await fetch(HEALTH_PATH, { method: "GET" });
      if (!res.ok) {
        setHealthState("error", "ヘルス異常 HTTP " + res.status);
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
        setHealthState("ok", "ヘルス正常");
      } else {
        setHealthState("error", "ヘルス応答異常");
      }
    } catch (err) {
      setHealthState("error", "ヘルス未到達");
    } finally {
      healthInFlight = false;
    }
  }

  function selectedFormat() {
    var format = (formatSelect.value || "json").toLowerCase();
    if (format !== "csv" && format !== "json") {
      return "json";
    }
    return format;
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
    // JSON is the console default; always send an explicit format.
    form.append("format", selectedFormat());

    setBusy(true);
    try {
      var headers = {};
      if (selectedFormat() === "json") {
        headers.Accept = "application/json";
      } else {
        headers.Accept = "text/csv";
      }
      var res = await fetch(ANALYZE_PATH, {
        method: "POST",
        body: form,
        headers: headers,
      });
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

  eclInput.addEventListener("change", function () {
    var file = eclInput.files && eclInput.files[0];
    eclFilename.textContent = file ? file.name : "未選択";
  });

  analyzeBtn.addEventListener("click", function () {
    if (busy) {
      return;
    }
    onAnalyze();
  });

  downloadBtn.addEventListener("click", onDownload);

  // Immediate health check, then every 10 seconds (no button).
  setHealthState("pending", "ヘルス確認中…");
  pollHealth();
  setInterval(pollHealth, HEALTH_POLL_MS);
})();
