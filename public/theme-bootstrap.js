/* global document, localStorage */

try {
  document.documentElement.dataset.theme = localStorage.getItem('jevcode-theme') || 'dark';
} catch {
  document.documentElement.dataset.theme = 'dark';
}
