export type ThemePreference = 'system' | 'light' | 'dark';
export type ResolvedTheme = 'light' | 'dark';

export const THEME_PREFERENCE_KEY = 'kuku.theme.preference';

export function resolveTheme(input: {
  preference: ThemePreference;
  system: ResolvedTheme;
}): ResolvedTheme {
  return input.preference === 'system' ? input.system : input.preference;
}

export function readThemePreference(storage: Pick<Storage, 'getItem'>): ThemePreference {
  const value = storage.getItem(THEME_PREFERENCE_KEY);
  return value === 'light' || value === 'dark' ? value : 'system';
}

export function writeThemePreference(
  storage: Pick<Storage, 'setItem'>,
  preference: ThemePreference,
): void {
  storage.setItem(THEME_PREFERENCE_KEY, preference);
}
