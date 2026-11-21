(function install(tools, methods, encode, decode, lineLimit) {
    for (const {name} of methods) {
        const call = tools[name];
        tools[name] = async (args = {}) => call(args);
    }
    tools.methods = Object.freeze(methods);
    const read = tools.read_output;
    const readSchema = methods.find(({name}) => name === "read_output")?.input_schema;
    const defaultLimit = readSchema?.properties?.limit?.default ?? 8192;
    const textLimit = 1024 * 1024;

    if (tools.run_command) {
        const run = tools.run_command;
        tools.run_command = async (args) => {
            const response = await run(args);
            const facts = response.result;
            if (response.status !== "ok" || facts?.termination !== "exited" || !Number.isInteger(facts.exit_code)) {
                const detail = response.error ?? (facts?.signal == null
                    ? "exit code unavailable" : `process terminated by signal ${facts.signal}`);
                const error = new Error(`run_command: ${detail}`);
                error.status = response.status === "ok" ? "error" : response.status;
                error.termination = facts?.termination;
                error.signal = facts?.signal;
                error.output_ref = facts?.output_ref;
                throw error;
            }
            const output_ref = Object.freeze(facts.output_ref);
            const capture_complete = facts.capture_complete;
            const inline = facts.inline_pipes;
            return Object.freeze({
                exit_code: facts.exit_code,
                duration_ms: facts.duration_ms,
                output_ref,
                capture_complete,
                stdout: commandStream("stdout", facts.stdout_bytes, output_ref, capture_complete, inline?.stdout),
                stderr: commandStream("stderr", facts.stderr_bytes, output_ref, capture_complete, inline?.stderr),
            });
        };
    }

    async function* output_chunks({offset = 0, limit = defaultLimit, ...source}) {
        while (true) {
            const {status, error, result} = await read({...source, offset, limit});
            if (status !== "ok") throw new Error(error);
            yield result;
            if (!result.has_more) return;
            if (result.next_offset <= offset) throw new Error("read_output did not advance");
            offset = result.next_offset;
            if (source.name !== undefined) source = {id: result.output_id, search: source.search};
        }
    }

    async function* output_lines({stream, offset = 0, search, ...source}) {
        if (stream !== undefined && stream !== "stdout" && stream !== "stderr") {
            throw new TypeError("output_lines stream must be stdout or stderr");
        }
        if (search !== undefined) throw new TypeError("output_lines requires unfiltered output");
        yield* linesFromChunks(output_chunks({...source, offset}), stream, offset);
    }

    async function* linesFromChunks(chunks, stream, offset, checkLength) {
        const pending = new Map();
        let lastChunk;
        for await (const chunk of chunks) {
            lastChunk = chunk;
            if (chunk.filtered) throw new Error("output_lines requires unfiltered output");
            for (const segment of chunk.segments) {
                if (stream !== undefined && segment.stream !== stream) continue;
                if (!pending.has(segment.stream)) {
                    pending.set(segment.stream, {
                        buffer: new Uint8Array(lineLimit),
                        startsLine: offset === 0,
                        bytes: 0,
                    });
                }
                const line = pending.get(segment.stream);
                const raw = segmentBytes(segment);
                let start = 0;
                while (start < raw.length) {
                    const lf = raw.indexOf(10, start);
                    const end = lf < 0 ? raw.length : lf + 1;
                    if (!line.bytes) line.offset = segment.offset + start;
                    if (line.bytes < lineLimit) {
                        const keep = Math.min(end - start, lineLimit - line.bytes);
                        line.buffer.set(raw.subarray(start, start + keep), line.bytes);
                    }
                    line.bytes += end - start;
                    if (checkLength) checkLength(line.bytes);
                    line.nextOffset = segment.offset + end;
                    start = end;
                    if (lf >= 0) {
                        yield renderLine(segment.stream, line, chunk, true);
                        line.bytes = 0;
                        line.startsLine = true;
                    }
                }
            }
        }
        for (const [stream, line] of pending) {
            if (line.bytes) yield renderLine(stream, line, lastChunk, false);
        }
    }

    function segmentBytes(segment) {
        return segment.raw_bytes ? Uint8Array.from(segment.raw_bytes) : encode(segment.text);
    }

    function commandStream(stream, size, reference, captureComplete, inline) {
        const rawHint = read ? " Use tools.output_chunks for raw bytes." : " Give the retained output reference to the host for inspection.";

        function failure(reason) {
            const error = new Error(`${stream}: ${reason}`);
            error.output_ref = reference;
            throw error;
        }

        function checkCapture() {
            if (!captureComplete) failure(`capture is incomplete.${rawHint}`);
            if (size > 0 && !inline && !read) failure("tools.read_output is required to read retained command output");
        }

        async function* chunks() {
            if (inline) {
                yield {complete: true, segments: [{stream, offset: reference.offset, raw_bytes: inline}]};
                return;
            }
            let offset = reference.offset;
            let observed = 0;
            for await (const chunk of output_chunks({id: reference.id, offset: reference.offset})) {
                if (!chunk.complete || chunk.capture_truncated || chunk.cleanup_incomplete || chunk.filtered) {
                    failure(`capture is incomplete.${rawHint}`);
                }
                if (chunk.output_id !== reference.id || chunk.offset !== offset || chunk.total_bytes !== reference.offset + reference.bytes) {
                    failure("retained output does not match the command result");
                }
                offset = chunk.next_offset;
                for (const segment of chunk.segments) {
                    if (segment.stream === stream) observed += segment.bytes;
                }
                if (observed > size) failure("retained output size differs from the command result");
                yield chunk;
                if (observed === size) return;
            }
        }

        async function text() {
            checkCapture();
            if (size > textLimit) failure(`text exceeds ${textLimit} bytes. Use ${stream}.lines().`);
            if (size === 0) return "";
            const bytes = new Uint8Array(size);
            let written = 0;
            for await (const chunk of chunks()) {
                for (const segment of chunk.segments) {
                    if (segment.stream !== stream) continue;
                    const raw = segmentBytes(segment);
                    if (written + raw.length > size) failure("retained output size differs from the command result");
                    bytes.set(raw, written);
                    written += raw.length;
                }
            }
            if (written !== size) failure("retained output size differs from the command result");
            const value = decode(bytes, true, true);
            if (value.lossy_utf8) failure(`output could not be decoded losslessly.${rawHint}`);
            return value.text;
        }

        async function json() {
            return JSON.parse(await text());
        }

        async function* lines() {
            checkCapture();
            if (size === 0) return;
            let total = 0;
            const checkLength = (bytes) => {
                if (bytes > lineLimit) failure(`line exceeds ${lineLimit} bytes.${rawHint}`);
            };
            for await (const line of linesFromChunks(chunks(), stream, reference.offset, checkLength)) {
                if (line.lossy_utf8) failure(`output could not be decoded losslessly.${rawHint}`);
                total += line.bytes;
                yield line.text;
            }
            if (total !== size) failure("retained output size differs from the command result");
        }

        return Object.freeze({text, json, lines});
    }

    function renderLine(stream, line, chunk, endedWithLf) {
        const bytes = line.buffer.subarray(0, Math.min(line.bytes, lineLimit));
        const truncated = line.bytes > lineLimit;
        const {text, lossy_utf8} = decode(bytes, line.startsLine, !truncated && (endedWithLf || chunk.complete));
        return {
            text: endedWithLf && !truncated ? text.replace(/\r?\n$/, "") : text,
            stream,
            offset: line.offset,
            next_offset: line.nextOffset,
            bytes: line.bytes,
            lossy_utf8,
            truncated,
            complete: chunk.complete,
            capture_truncated: chunk.capture_truncated,
        };
    }

    if (read) {
        tools.output_chunks = output_chunks;
        tools.output_lines = output_lines;
    }
    return Object.freeze(tools);
})
