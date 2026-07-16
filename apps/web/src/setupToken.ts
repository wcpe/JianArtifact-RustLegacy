const SETUP_TOKEN_FRAGMENT = /^#token=([^&]+)$/;

export function consumeSetupToken(): string | null {
  const fragment = window.location.hash;
  window.history.replaceState(
    window.history.state,
    '',
    `${window.location.pathname}${window.location.search}`,
  );

  const encodedToken = SETUP_TOKEN_FRAGMENT.exec(fragment)?.[1];
  if (!encodedToken) return null;
  try {
    return decodeURIComponent(encodedToken);
  } catch {
    return null;
  }
}
