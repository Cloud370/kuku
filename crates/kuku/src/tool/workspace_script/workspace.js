(function install(workspace, methods, encode, decode, lineLimit) {
    for (const {name} of methods) {
        const call = workspace[name];
        workspace[name] = async (args = {}) => call(args);
    }
    workspace.methods = Object.freeze(methods);
    const read = workspace.read_output;
    if (!read) return Object.freeze(workspace);
    const schema = methods.find(({name}) => name === "read_output").input_schema;
    const defaultLimit = Math.min(65536, schema.properties?.limit?.maximum ?? 8192);

    async function* output_chunks({offset = 0, limit = defaultLimit, ...source}) {
        while (true) {
            const {status, summary, result, model_content} = await read({...source, offset, limit});
            if (status !== "ok") throw new Error(model_content || summary);
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
        const pending = new Map();
        let lastChunk;
        for await (const chunk of output_chunks({...source, offset})) {
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
                const raw = segment.raw_bytes ? Uint8Array.from(segment.raw_bytes) : encode(segment.text);
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

    workspace.output_chunks = output_chunks;
    workspace.output_lines = output_lines;
    return Object.freeze(workspace);
})
