(function () {
    "use strict";

    const container = document.getElementById("container");
    const statusEl = document.getElementById("connection-status");
    const MAX_LOG_LINES = 2000;
    let socket = null;

    const icons = {
        play: '<path d="M6 4l12 8-12 8z"/>',
        stop: '<rect x="6" y="6" width="12" height="12" rx="1"/>',
        send: '<path d="M22 2L11 13"/><path d="M22 2L15 22l-4-9-9-4z"/>',
        terminal: '<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>',
    };

    function icon(name) {
        return `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${icons[name]}</svg>`;
    }

    function connect() {
        const proto = location.protocol === "https:" ? "wss:" : "ws:";
        socket = new WebSocket(`${proto}//${location.host}/ws`);

        socket.onopen = () => setStatus("connected", "Connected");
        socket.onclose = () => {
            setStatus("disconnected", "Reconnecting\u2026");
            setTimeout(connect, 2000);
        };
        socket.onerror = () => socket.close();
        socket.binaryType = "arraybuffer";
        socket.onmessage = (e) => {
            const text = e.data instanceof ArrayBuffer
                ? new TextDecoder().decode(e.data)
                : e.data;
            handleMessage(JSON.parse(text));
        };
    }

    function setStatus(cls, text) {
        statusEl.className = cls;
        statusEl.textContent = text;
    }

    function handleMessage(msg) {
        switch (msg.type) {
            case "Init":
                container.innerHTML = "";
                msg.services.forEach((s, i) => createServiceBox(s, i));
                break;
            case "Log":
                appendLog(msg.idx, msg.line);
                break;
            case "Status":
                updateStatus(msg.idx, msg.status);
                break;
        }
    }

    const ANSI_RE = /\x1b\[([0-9;]*)m/g;
    const BASIC_FG = ["#000","#c23621","#25bc24","#adad27","#492ee1","#d338d3","#33bbc8","#cbcccd"];
    const BRIGHT_FG = ["#666","#f14c4c","#23d18b","#f5f543","#3b8eea","#d670d6","#29b8db","#e5e5e5"];

    function ansiToHtml(text) {
        let out = "";
        let last = 0;
        let fg = null, bg = null, bold = false, dim = false, italic = false, underline = false;

        function openSpan() {
            const parts = [];
            if (fg) parts.push("color:" + fg);
            if (bg) parts.push("background:" + bg);
            if (bold) parts.push("font-weight:bold");
            if (dim) parts.push("opacity:0.6");
            if (italic) parts.push("font-style:italic");
            if (underline) parts.push("text-decoration:underline");
            return parts.length ? '<span style="' + parts.join(";") + '">' : "";
        }

        function esc(s) {
            return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
        }

        let hasStyle = false;
        let match;
        while ((match = ANSI_RE.exec(text)) !== null) {
            const chunk = text.slice(last, match.index);
            if (chunk) {
                if (!hasStyle) { out += openSpan(); hasStyle = true; }
                out += esc(chunk);
            }
            last = match.index + match[0].length;

            const codes = match[1] ? match[1].split(";").map(Number) : [0];
            for (let i = 0; i < codes.length; i++) {
                const c = codes[i];
                if (c === 0) { if (hasStyle) { out += "</span>"; hasStyle = false; } fg = bg = null; bold = dim = italic = underline = false; }
                else if (c === 1) { if (hasStyle) { out += "</span>"; hasStyle = false; } bold = true; }
                else if (c === 2) { if (hasStyle) { out += "</span>"; hasStyle = false; } dim = true; }
                else if (c === 3) { if (hasStyle) { out += "</span>"; hasStyle = false; } italic = true; }
                else if (c === 4) { if (hasStyle) { out += "</span>"; hasStyle = false; } underline = true; }
                else if (c === 22) { if (hasStyle) { out += "</span>"; hasStyle = false; } bold = dim = false; }
                else if (c === 23) { if (hasStyle) { out += "</span>"; hasStyle = false; } italic = false; }
                else if (c === 24) { if (hasStyle) { out += "</span>"; hasStyle = false; } underline = false; }
                else if (c >= 30 && c <= 37) { if (hasStyle) { out += "</span>"; hasStyle = false; } fg = (bold ? BRIGHT_FG : BASIC_FG)[c - 30]; }
                else if (c === 39) { if (hasStyle) { out += "</span>"; hasStyle = false; } fg = null; }
                else if (c >= 40 && c <= 47) { if (hasStyle) { out += "</span>"; hasStyle = false; } bg = BASIC_FG[c - 40]; }
                else if (c === 49) { if (hasStyle) { out += "</span>"; hasStyle = false; } bg = null; }
                else if (c >= 90 && c <= 97) { if (hasStyle) { out += "</span>"; hasStyle = false; } fg = BRIGHT_FG[c - 90]; }
                else if (c >= 100 && c <= 107) { if (hasStyle) { out += "</span>"; hasStyle = false; } bg = BRIGHT_FG[c - 100]; }
                else if (c === 38 || c === 48) {
                    const isFg = c === 38;
                    if (hasStyle) { out += "</span>"; hasStyle = false; }
                    if (codes[i + 1] === 5 && codes.length > i + 2) {
                        const n = codes[i + 2];
                        if (isFg) fg = color256(n); else bg = color256(n);
                        i += 2;
                    } else if (codes[i + 1] === 2 && codes.length > i + 4) {
                        const hex = "#" + [codes[i+2],codes[i+3],codes[i+4]].map(v => (v&0xff).toString(16).padStart(2,"0")).join("");
                        if (isFg) fg = hex; else bg = hex;
                        i += 4;
                    }
                }
            }
        }

        const tail = text.slice(last);
        if (tail) {
            if (!hasStyle) { out += openSpan(); hasStyle = true; }
            out += esc(tail);
        }
        if (hasStyle) out += "</span>";
        return out || esc(text);
    }

    function color256(n) {
        if (n < 8) return BASIC_FG[n];
        if (n < 16) return BRIGHT_FG[n - 8];
        if (n >= 232) { const g = 8 + (n - 232) * 10; return `rgb(${g},${g},${g})`; }
        n -= 16;
        const r = Math.floor(n / 36), g = Math.floor((n % 36) / 6), b = n % 6;
        return `rgb(${r ? r * 40 + 55 : 0},${g ? g * 40 + 55 : 0},${b ? b * 40 + 55 : 0})`;
    }

    function appendLog(idx, line) {
        const log = document.getElementById(`log-${idx}`);
        if (!log) return;
        const el = document.createElement("div");
        el.innerHTML = ansiToHtml(line);
        log.appendChild(el);
        if (log.childNodes.length > MAX_LOG_LINES) {
            log.removeChild(log.firstChild);
        }
        log.scrollTop = log.scrollHeight;
    }

    function updateStatus(idx, status) {
        const badge = document.getElementById(`badge-${idx}`);
        const startBtn = document.getElementById(`start-${idx}`);
        const stopBtn = document.getElementById(`stop-${idx}`);

        if (badge) {
            badge.className = `status-badge ${status}`;
            badge.textContent = status.toLowerCase();
        }

        const busy = status === "Starting" || status === "Stopping";
        if (startBtn) startBtn.disabled = status === "Running" || busy;
        if (stopBtn) stopBtn.disabled = status === "Stopped" || busy;
    }

    function createServiceBox(svc, idx) {
        const box = document.createElement("div");
        box.className = "service-box";

        const header = document.createElement("div");
        header.className = "service-header";

        const name = document.createElement("div");
        name.className = "service-name";

        const badge = document.createElement("span");
        badge.id = `badge-${idx}`;
        badge.className = `status-badge ${svc.status}`;
        badge.textContent = svc.status.toLowerCase();

        const termIcon = document.createElement("span");
        termIcon.className = "service-icon";
        termIcon.innerHTML = icon("terminal");
        const label = document.createTextNode(svc.name);
        name.append(termIcon, label, badge);

        const controls = document.createElement("div");
        controls.className = "controls";

        const startBtn = document.createElement("button");
        startBtn.id = `start-${idx}`;
        startBtn.innerHTML = `${icon("play")} Start`;
        startBtn.disabled = svc.status === "Running";
        startBtn.onclick = () => sendCommand(idx, "start");

        const stopBtn = document.createElement("button");
        stopBtn.id = `stop-${idx}`;
        stopBtn.innerHTML = `${icon("stop")} Stop`;
        stopBtn.disabled = svc.status === "Stopped";
        stopBtn.onclick = () => sendCommand(idx, "stop");

        controls.append(startBtn, stopBtn);
        header.append(name, controls);

        const logArea = document.createElement("div");
        logArea.className = "log-area";
        logArea.id = `log-${idx}`;

        box.append(header, logArea);

        if (svc.interactive) {
            const stdin = document.createElement("div");
            stdin.className = "stdin-area";

            const input = document.createElement("input");
            input.type = "text";
            input.placeholder = "Type command\u2026";
            input.onkeydown = (e) => {
                if (e.key === "Enter") sendStdin(idx, input);
            };

            const sendBtn = document.createElement("button");
            sendBtn.innerHTML = `${icon("send")} Send`;
            sendBtn.onclick = () => sendStdin(idx, input);

            stdin.append(input, sendBtn);
            box.appendChild(stdin);
        }

        container.appendChild(box);

        for (const line of svc.logs) {
            appendLog(idx, line);
        }
    }

    function send(obj) {
        if (socket && socket.readyState === WebSocket.OPEN) {
            socket.send(new TextEncoder().encode(JSON.stringify(obj)));
        }
    }

    function sendCommand(idx, command) {
        send({ type: "Command", idx, command });
    }

    function sendStdin(idx, input) {
        if (!input.value) return;
        send({ type: "Command", idx, command: "stdin", input: input.value });
        input.value = "";
    }

    connect();
})();