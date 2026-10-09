export function shortcutModifier(platform: string): '⌘' | 'Ctrl' {
  return /Mac|iPhone|iPad/i.test(platform) ? '⌘' : 'Ctrl';
}

export const primaryShortcutModifier = shortcutModifier(typeof navigator === 'undefined' ? '' : navigator.platform);
