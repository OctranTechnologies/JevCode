export function Brand({ large = false }: { large?: boolean }) {
  return <span className={`brand-mark ${large ? 'brand-mark-large' : ''}`} aria-hidden="true">J</span>;
}
