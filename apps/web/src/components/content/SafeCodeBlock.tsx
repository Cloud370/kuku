import { Check, Copy } from 'lucide-react';
import DOMPurify from 'dompurify';
import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import markdown from 'highlight.js/lib/languages/markdown';
import rust from 'highlight.js/lib/languages/rust';
import typescript from 'highlight.js/lib/languages/typescript';
import { useState } from 'react';

hljs.registerLanguage('bash', bash);
hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('json', json);
hljs.registerLanguage('markdown', markdown);
hljs.registerLanguage('rust', rust);
hljs.registerLanguage('typescript', typescript);

const LANGUAGE_ALIASES: Record<string, string> = {
  js: 'javascript',
  md: 'markdown',
  rs: 'rust',
  sh: 'bash',
  ts: 'typescript',
};

interface SafeCodeBlockProps {
  code: string;
  language?: string | null;
}

function hasClipboard(value: unknown): value is Pick<Clipboard, 'writeText'> {
  return (
    typeof value === 'object' &&
    value !== null &&
    'writeText' in value &&
    typeof value.writeText === 'function'
  );
}

export function SafeCodeBlock({ code, language = null }: SafeCodeBlockProps) {
  const [copyStatus, setCopyStatus] = useState<'idle' | 'copied' | 'unavailable'>('idle');
  const requestedLanguage = language?.trim().toLowerCase() ?? '';
  const normalizedLanguage = LANGUAGE_ALIASES[requestedLanguage] ?? requestedLanguage;
  const registered =
    normalizedLanguage.length > 0 && hljs.getLanguage(normalizedLanguage) !== undefined;
  const highlighted = registered
    ? DOMPurify.sanitize(hljs.highlight(code, { language: normalizedLanguage }).value, {
        ALLOWED_ATTR: ['class'],
        ALLOWED_TAGS: ['span'],
      })
    : null;
  const label = requestedLanguage.length > 0 ? `${requestedLanguage} code` : 'Code';

  async function copy() {
    const clipboardValue = (navigator as unknown as { clipboard?: unknown }).clipboard;
    if (!hasClipboard(clipboardValue)) {
      setCopyStatus('unavailable');
      return;
    }
    try {
      await clipboardValue.writeText(code);
      setCopyStatus('copied');
    } catch {
      setCopyStatus('unavailable');
    }
  }

  return (
    <section
      aria-label={label}
      className="my-3 max-h-96 overflow-auto rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface)]"
      role="region"
    >
      <div className="sticky top-0 flex h-9 items-center justify-between border-b border-[var(--color-border)] bg-[var(--color-surface-raised)] px-2">
        <span className="text-xs text-[var(--color-text-muted)]">
          {requestedLanguage.length > 0 ? requestedLanguage : 'text'}
        </span>
        <button
          aria-label="Copy code"
          className="inline-flex size-7 items-center justify-center rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]"
          onClick={() => void copy()}
          title="Copy code"
          type="button"
        >
          {copyStatus === 'copied' ? (
            <Check aria-hidden="true" size={14} />
          ) : (
            <Copy aria-hidden="true" size={14} />
          )}
        </button>
      </div>
      <pre className="m-0 overflow-visible p-4 text-xs">
        {highlighted === null ? (
          <code>{code}</code>
        ) : (
          <code
            className={`hljs language-${normalizedLanguage}`}
            dangerouslySetInnerHTML={{ __html: highlighted }}
          />
        )}
      </pre>
      {copyStatus !== 'idle' ? (
        <span className="sr-only" role="status">
          {copyStatus === 'copied' ? 'Copied' : 'Copy unavailable'}
        </span>
      ) : null}
    </section>
  );
}
