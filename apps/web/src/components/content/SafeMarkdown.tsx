import DOMPurify from 'dompurify';
import { marked, type Token, type Tokens } from 'marked';

import { SafeCodeBlock } from './SafeCodeBlock';

interface SafeMarkdownProps {
  source: string;
}

function safeHtml(token: Token): string {
  const parsed = marked.parser([token], { async: false });
  const sanitized = DOMPurify.sanitize(parsed, {
    FORBID_ATTR: ['style'],
    FORBID_TAGS: ['audio', 'form', 'iframe', 'img', 'object', 'script', 'source', 'style', 'video'],
  });
  const documentFragment = new DOMParser().parseFromString(sanitized, 'text/html');
  const links = Array.from(documentFragment.querySelectorAll<HTMLAnchorElement>('a'));
  for (const link of links) {
    const href = link.getAttribute('href');
    if (href === null) continue;
    try {
      const url = new URL(href, location.origin);
      if (url.protocol !== 'http:' && url.protocol !== 'https:') {
        link.removeAttribute('href');
      } else if (url.origin !== location.origin) {
        link.setAttribute('rel', 'noopener noreferrer');
        link.setAttribute('target', '_blank');
      }
    } catch {
      link.removeAttribute('href');
    }
  }
  return documentFragment.body.innerHTML;
}

function isCodeToken(token: Token): token is Tokens.Code {
  return token.type === 'code';
}

export function SafeMarkdown({ source }: SafeMarkdownProps) {
  const tokens = marked.lexer(source);
  return (
    <div className="prose max-w-none break-words">
      {tokens.map((token, index) =>
        isCodeToken(token) ? (
          <SafeCodeBlock
            code={token.text}
            key={`${token.type}-${String(index)}`}
            language={token.lang}
          />
        ) : (
          <div
            dangerouslySetInnerHTML={{ __html: safeHtml(token) }}
            key={`${token.type}-${String(index)}`}
          />
        ),
      )}
    </div>
  );
}
