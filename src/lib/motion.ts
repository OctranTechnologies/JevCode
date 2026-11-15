/** Smooth scrolling is replaced by an instant jump when the user has asked for reduced motion. */
export function scrollBehavior(preferred: ScrollBehavior): ScrollBehavior {
  if (typeof window === 'undefined' || !window.matchMedia) return preferred;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : preferred;
}
