(function installConsole(maxResultBytes) {
    const suffix = "\n[console output truncated]";
    const maxJsonBytesPerCodeUnit = 6;
    const limit = Math.floor((maxResultBytes - JSON.stringify(suffix).length) / maxJsonBytesPerCodeUnit);
    let text = "";
    let logged = false;
    let truncated = false;

    function append(value) {
        const remaining = limit - text.length;
        truncated = value.length > remaining;
        text += value.slice(0, remaining).replace(/[\uD800-\uDFFF]/gu, "\uFFFD");
    }

    function display(value) {
        try {
            if (value !== null && typeof value === "object") {
                return JSON.stringify(value) ?? String(value);
            }
            return String(value);
        } catch {
            return "[unprintable]";
        }
    }

    function log(...values) {
        if (truncated) return;
        if (logged) append("\n");
        logged = true;
        for (let index = 0; index < values.length && !truncated; index++) {
            if (index > 0) append(" ");
            if (!truncated) append(display(values[index]));
        }
    }

    Object.defineProperty(globalThis, "console", {value: Object.freeze({log})});
    return () => logged ? text + (truncated ? suffix : "") : undefined;
})
