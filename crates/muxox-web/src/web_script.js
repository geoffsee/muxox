(function () {
    "use strict";

    const container = document.getElementById("container");
    const statusEl = document.getElementById("connection-status");
    const MAX_LOG_LINES = 2000;
    const LAYOUT_KEY = "muxox.shellLayout.v1";
    const MIN_COL_W = 320;
    const MIN_CARD_H = 140;
    const GUTTER = 12;
    const MIN_COL_TRACK = 420;
    let socket = null;
    let savedLayouts = loadLayouts();
    let session = null;
    let resizeLock = 0;
    let appliedBudget = -1;
    let layoutTick = 0;

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
                mountShells(msg.services.map((s, i) => createServiceBox(s, i)));
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

    function writeLog(log, line) {
        const el = document.createElement("div");
        el.innerHTML = ansiToHtml(line);
        log.appendChild(el);
        if (log.childNodes.length > MAX_LOG_LINES) {
            log.removeChild(log.firstChild);
        }
        log.scrollTop = log.scrollHeight;
    }

    function appendLog(idx, line) {
        const log = document.getElementById(`log-${idx}`);
        if (!log) return;
        writeLog(log, line);
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
        box.dataset.idx = String(idx);
        box.dataset.name = svc.name || ("service-" + idx);

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
        const label = document.createElement("span");
        label.className = "service-label";
        label.textContent = svc.name;
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

        for (const line of svc.logs) {
            writeLog(logArea, line);
        }

        return box;
    }

    // Shells tile the viewport. Vertical splitters resize columns; horizontal
    // splitters trade height between the two shells stacked in that column.
    function loadLayouts() {
        try {
            const raw = localStorage.getItem(LAYOUT_KEY);
            const data = raw ? JSON.parse(raw) : null;
            if (!data || typeof data !== "object" || Array.isArray(data)) return {};
            return data;
        } catch (e) {
            return {};
        }
    }

    function metrics() {
        const cs = getComputedStyle(container);
        const padX = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
        const padY = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
        return {
            width: Math.max(0, container.clientWidth - padX),
            height: Math.max(0, container.clientHeight - padY),
        };
    }

    function chooseCols(n) {
        if (n <= 1) return 1;
        const { width } = metrics();
        let cols = 1;
        while (cols < n) {
            const next = cols + 1;
            if (next * MIN_COL_TRACK + (next - 1) * GUTTER > width) break;
            cols = next;
        }
        return cols;
    }

    function rowBudget(rows) {
        const { height } = metrics();
        const gutters = GUTTER * Math.max(0, rows - 1);
        return Math.max(rows * MIN_CARD_H, height - gutters);
    }

    function sortedBoxes() {
        return [...container.querySelectorAll(".service-box")].sort((a, b) => Number(a.dataset.idx) - Number(b.dataset.idx));
    }

    function clampPair(a, b, delta, min) {
        const total = a + b;
        if (total <= min * 2) return [a, b];
        const next = Math.max(min, Math.min(total - min, a + delta));
        return [next, total - next];
    }

    function setSeparatorValue(el, a, b) {
        const total = a + b;
        const pct = total > 0 ? Math.round((a / total) * 100) : 50;
        el.setAttribute("aria-valuemin", "0");
        el.setAttribute("aria-valuemax", "100");
        el.setAttribute("aria-valuenow", String(pct));
    }

    function columnCards(col) {
        return [...col.children].filter((el) => el.classList.contains("service-box"));
    }

    function writeColumnFracs(col) {
        const cards = columnCards(col);
        const total = cards.reduce((sum, card) => sum + card.offsetHeight, 0) || 1;
        cards.forEach((card) => {
            session.cardFrac[card.dataset.name] = card.offsetHeight / total;
        });
    }

    function applyColWeights() {
        const cols = [...container.querySelectorAll(".shell-col")];
        cols.forEach((col, i) => {
            const w = Number(session.colWeights[i]);
            col.style.flex = `${Number.isFinite(w) && w > 0 ? w : 1} 1 0px`;
        });
    }

    // Weights start equal (all 1). Once a boundary moves, every column needs a
    // pixel weight or the untouched ones collapse toward zero.
    function materializeColWeights() {
        if (!session) return;
        [...container.querySelectorAll(".shell-col")].forEach((col, i) => {
            const w = col.getBoundingClientRect().width;
            if (w > 0) session.colWeights[i] = w;
        });
    }

    function applyHeights() {
        if (!session) return;
        const rows = session.rows;
        const budget = rowBudget(rows);
        const slot = budget / rows;
        container.querySelectorAll(".shell-col").forEach((col) => {
            const cards = columnCards(col);
            if (!cards.length) return;
            const owned = slot * cards.length;
            let fracs = cards.map((card) => {
                const f = Number(session.cardFrac[card.dataset.name]);
                return Number.isFinite(f) && f > 0 ? f : 1;
            });
            const sum = fracs.reduce((a, b) => a + b, 0) || 1;
            fracs = fracs.map((f) => f / sum);
            let used = 0;
            cards.forEach((card, i) => {
                let h = i === cards.length - 1 ? Math.round(owned - used) : Math.round(fracs[i] * owned);
                if (h < MIN_CARD_H) h = MIN_CARD_H;
                used += h;
                card.style.height = h + "px";
            });
        });
        appliedBudget = budget;
    }

    function refreshSeparatorValues() {
        container.querySelectorAll(".splitter-h").forEach((el) => {
            const prev = el.previousElementSibling;
            const next = el.nextElementSibling;
            if (prev && next) setSeparatorValue(el, prev.offsetHeight, next.offsetHeight);
        });
        const cols = [...container.querySelectorAll(".shell-col")];
        container.querySelectorAll(".splitter-v").forEach((el) => {
            const index = Number(el.dataset.index);
            const left = cols[index];
            const right = cols[index + 1];
            if (left && right) setSeparatorValue(el, left.getBoundingClientRect().width, right.getBoundingClientRect().width);
        });
    }

    function persist() {
        if (!session) return;
        const key = session.cols + "x" + session.rows;
        savedLayouts[key] = {
            colWeights: session.colWeights.slice(),
            cardFrac: { ...session.cardFrac },
        };
        try {
            localStorage.setItem(LAYOUT_KEY, JSON.stringify(savedLayouts));
        } catch (e) { /* private browsing */ }
    }

    function layoutFor(cols, rows) {
        const key = cols + "x" + rows;
        const existing = savedLayouts[key];
        if (existing && Array.isArray(existing.colWeights) && existing.colWeights.length === cols) {
            return {
                cols,
                rows,
                colWeights: existing.colWeights.map(Number),
                cardFrac: existing.cardFrac && typeof existing.cardFrac === "object" ? { ...existing.cardFrac } : {},
            };
        }
        return { cols, rows, colWeights: Array(cols).fill(1), cardFrac: {} };
    }

    function makeSplitter(kind) {
        const el = document.createElement("div");
        el.className = "splitter splitter-" + kind;
        el.tabIndex = 0;
        el.setAttribute("role", "separator");
        el.setAttribute("aria-orientation", kind === "v" ? "vertical" : "horizontal");
        el.title = "Drag to resize. Arrow keys nudge. Double-click to reset.";
        return el;
    }

    function trackDrag(handle, event, kind, onMove) {
        if (event.button !== 0) return;
        event.preventDefault();
        resizeLock++;
        try { handle.setPointerCapture(event.pointerId); } catch (e) { /* pointer already gone */ }
        handle.classList.add("is-dragging");
        handle.focus({ preventScroll: true });
        document.body.classList.add(kind === "col" ? "is-resizing-col" : "is-resizing-row");
        const boxes = kind === "row"
            ? [handle.previousElementSibling, handle.nextElementSibling]
            : columnCards(handle.previousElementSibling).concat(columnCards(handle.nextElementSibling));
        const pins = boxes.filter(Boolean).map(capturePin);

        function move(ev) {
            onMove(ev);
            restorePin(pins);
        }
        function end() {
            window.removeEventListener("pointermove", move);
            window.removeEventListener("pointerup", end);
            window.removeEventListener("pointercancel", end);
            try {
                if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
            } catch (e) { /* capture already released */ }
            handle.classList.remove("is-dragging");
            document.body.classList.remove("is-resizing-col", "is-resizing-row");
            resizeLock = Math.max(0, resizeLock - 1);
            persist();
        }
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", end);
        window.addEventListener("pointercancel", end);
    }

    function capturePin(box) {
        const log = box.querySelector(".log-area");
        if (!log) return null;
        const distance = log.scrollHeight - log.scrollTop - log.clientHeight;
        return { log, pin: distance < 8 };
    }

    function restorePin(pins) {
        pins.forEach((pin) => {
            if (pin && pin.pin) pin.log.scrollTop = pin.log.scrollHeight;
        });
    }

    function startColResize(event) {
        const handle = event.currentTarget;
        const index = Number(handle.dataset.index);
        const cols = [...container.querySelectorAll(".shell-col")];
        const left = cols[index];
        const right = cols[index + 1];
        if (!left || !right) return;
        materializeColWeights();
        applyColWeights();
        const startX = event.clientX;
        const w1 = left.getBoundingClientRect().width;
        const w2 = right.getBoundingClientRect().width;
        trackDrag(handle, event, "col", (ev) => {
            const pair = clampPair(w1, w2, ev.clientX - startX, MIN_COL_W);
            session.colWeights[index] = pair[0];
            session.colWeights[index + 1] = pair[1];
            left.style.flex = `${pair[0]} 1 0px`;
            right.style.flex = `${pair[1]} 1 0px`;
            setSeparatorValue(handle, pair[0], pair[1]);
        });
    }

    function startRowResize(event) {
        const handle = event.currentTarget;
        const prev = handle.previousElementSibling;
        const next = handle.nextElementSibling;
        if (!prev || !next) return;
        const startY = event.clientY;
        const h1 = prev.offsetHeight;
        const h2 = next.offsetHeight;
        trackDrag(handle, event, "row", (ev) => {
            const pair = clampPair(h1, h2, ev.clientY - startY, MIN_CARD_H);
            prev.style.height = pair[0] + "px";
            next.style.height = pair[1] + "px";
            writeColumnFracs(handle.parentElement);
            setSeparatorValue(handle, pair[0], pair[1]);
        });
    }

    function nudgeCol(handle, delta) {
        const index = Number(handle.dataset.index);
        const cols = [...container.querySelectorAll(".shell-col")];
        const left = cols[index];
        const right = cols[index + 1];
        if (!left || !right || !session) return;
        materializeColWeights();
        const pair = clampPair(left.getBoundingClientRect().width, right.getBoundingClientRect().width, delta, MIN_COL_W);
        session.colWeights[index] = pair[0];
        session.colWeights[index + 1] = pair[1];
        applyColWeights();
        setSeparatorValue(handle, pair[0], pair[1]);
        persist();
    }

    function nudgeRow(handle, delta) {
        const prev = handle.previousElementSibling;
        const next = handle.nextElementSibling;
        if (!prev || !next || !session) return;
        const pair = clampPair(prev.offsetHeight, next.offsetHeight, delta, MIN_CARD_H);
        prev.style.height = pair[0] + "px";
        next.style.height = pair[1] + "px";
        writeColumnFracs(handle.parentElement);
        setSeparatorValue(handle, pair[0], pair[1]);
        persist();
    }

    function resetColPair(index) {
        if (!session) return;
        materializeColWeights();
        const a = Number(session.colWeights[index]) || 1;
        const b = Number(session.colWeights[index + 1]) || 1;
        const avg = (a + b) / 2;
        session.colWeights[index] = avg;
        session.colWeights[index + 1] = avg;
        applyColWeights();
        refreshSeparatorValues();
        persist();
    }

    function resetRowPair(splitter) {
        if (!session) return;
        const prev = splitter.previousElementSibling;
        const next = splitter.nextElementSibling;
        if (!prev || !next) return;
        const col = splitter.parentElement;
        writeColumnFracs(col);
        const fp = Number(session.cardFrac[prev.dataset.name]) || 1;
        const fn = Number(session.cardFrac[next.dataset.name]) || 1;
        const avg = (fp + fn) / 2;
        session.cardFrac[prev.dataset.name] = avg;
        session.cardFrac[next.dataset.name] = avg;
        applyHeights();
        refreshSeparatorValues();
        persist();
    }

    function onSplitterKey(event, kind) {
        const step = event.shiftKey ? 80 : 24;
        let delta = 0;
        if (kind === "col") {
            if (event.key === "ArrowLeft") delta = -step;
            else if (event.key === "ArrowRight") delta = step;
            else return;
        } else if (event.key === "ArrowUp") delta = -step;
        else if (event.key === "ArrowDown") delta = step;
        else return;
        event.preventDefault();
        if (kind === "col") nudgeCol(event.currentTarget, delta);
        else nudgeRow(event.currentTarget, delta);
    }

    function makeVSplitter(index) {
        const el = makeSplitter("v");
        el.dataset.index = String(index);
        el.setAttribute("aria-label", "Resize adjacent columns");
        el.addEventListener("pointerdown", startColResize);
        el.addEventListener("dblclick", () => resetColPair(index));
        el.addEventListener("keydown", (event) => onSplitterKey(event, "col"));
        return el;
    }

    function makeHSplitter() {
        const el = makeSplitter("h");
        el.setAttribute("aria-label", "Resize adjacent services");
        el.addEventListener("pointerdown", startRowResize);
        el.addEventListener("dblclick", () => resetRowPair(el));
        el.addEventListener("keydown", (event) => onSplitterKey(event, "row"));
        return el;
    }

    function mountShells(boxes) {
        if (!boxes.length) {
            container.replaceChildren();
            session = null;
            appliedBudget = -1;
            return;
        }
        const cols = chooseCols(boxes.length);
        const rows = Math.ceil(boxes.length / cols);
        session = layoutFor(cols, rows);
        const columns = Array.from({ length: cols }, () => []);
        boxes.forEach((box, i) => columns[i % cols].push(box));

        const frag = document.createDocumentFragment();
        columns.forEach((members, c) => {
            if (c > 0) frag.appendChild(makeVSplitter(c - 1));
            const col = document.createElement("div");
            col.className = "shell-col";
            members.forEach((box, mi) => {
                if (mi > 0) col.appendChild(makeHSplitter());
                col.appendChild(box);
            });
            frag.appendChild(col);
        });
        container.replaceChildren(frag);
        applyColWeights();
        applyHeights();
        refreshSeparatorValues();
    }

    function scheduleRelayout() {
        if (resizeLock) return;
        const tick = ++layoutTick;
        requestAnimationFrame(() => {
            if (tick !== layoutTick || resizeLock) return;
            const boxes = sortedBoxes();
            if (!boxes.length || !session) return;
            const cols = chooseCols(boxes.length);
            const rows = Math.ceil(boxes.length / cols);
            if (cols !== session.cols || rows !== session.rows) {
                mountShells(boxes);
                return;
            }
            const budget = rowBudget(rows);
            if (Math.abs(budget - appliedBudget) < 1) return;
            applyHeights();
            refreshSeparatorValues();
        });
    }

    if (typeof ResizeObserver !== "undefined") {
        new ResizeObserver(() => scheduleRelayout()).observe(container);
    } else {
        window.addEventListener("resize", scheduleRelayout);
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