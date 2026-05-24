export type BreadcrumbProps = {
  path: { id: string; label: string }[];
  onNavigate?: (id: string) => void;
};

export function Breadcrumb({ path, onNavigate }: BreadcrumbProps) {
  return (
    <nav className="flex items-center gap-1.5 px-4 py-2 border-b border-[var(--color-border)] bg-[var(--color-surface)] shrink-0">
      {path.map((item, i) => (
        <span key={item.id} className="flex items-center gap-1.5">
          {i > 0 && (
            <span className="text-[var(--text-xs)] text-[var(--color-text-muted)]">&#x203A;</span>
          )}
          <button
            onClick={() => onNavigate?.(item.id)}
            className="text-[var(--text-xs)] text-[var(--color-text-secondary)] hover:text-[var(--color-accent)] transition-colors cursor-pointer truncate max-w-[200px]"
            title={item.label}
          >
            {item.label}
          </button>
        </span>
      ))}
    </nav>
  );
}
