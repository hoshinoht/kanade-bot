/**
 * Clipboard writes with a manual fallback: the Clipboard API needs a secure
 * context (the tailnet is https; a plain-http origin is not), so callers show
 * the text selected for Ctrl/Cmd-C when this answers false.
 */
export async function copyText(text: string): Promise<boolean> {
  if (!globalThis.isSecureContext || !navigator.clipboard?.writeText) return false;
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
