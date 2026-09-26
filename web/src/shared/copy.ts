/** Copies text; falls back to selecting `fallbackNode` so the reader can copy it by hand. */
export async function copyText(text: string, fallbackNode?: Node): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    if (fallbackNode) {
      const range = document.createRange();
      range.selectNodeContents(fallbackNode);
      const selection = getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
    return false;
  }
}
