/** The short message above the Flow ring (errors, hints). */
export const notice = $state<{ text: string | null }>({ text: null });

let timer: ReturnType<typeof setTimeout> | undefined;

/** Shows `text` for 5 s. */
export function notify(text: string): void {
  notice.text = text;
  clearTimeout(timer);
  timer = setTimeout(() => (notice.text = null), 5000);
}
