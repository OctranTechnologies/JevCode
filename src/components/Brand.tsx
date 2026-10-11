export function Brand({ large = false }: { large?: boolean }) {
  return (
    <span className={`brand-mark ${large ? 'brand-mark-large' : ''}`} aria-hidden="true">
      <svg viewBox="0 0 24 24" focusable="false">
        <path d="M12 12V3.3M12 12l8.29-2.68M12 12l5.12 7.05M12 12l-5.12 7.05M12 12L3.71 9.32" />
      </svg>
    </span>
  );
}
