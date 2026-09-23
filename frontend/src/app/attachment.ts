// What to call an attachment, shared by the thread (thread.html) and the
// clipboard (copy-log.ts). The screen shows an icon, so it prints the name
// alone; a pasted log prints the noun too.

import { Attachment } from './models';

/** An empty name is no name; `??` would keep the empty string. */
const named = (s: string | null): string | null => (s != null && s !== '' ? s : null);

/** What kind of thing it is, and so what to call it without a name. */
export function attachmentNoun(a: Attachment): string {
  return a.is_image ? 'image' : 'attachment';
}

/** What to call it, or null when nothing says more than the noun. The content
 *  type stands in for a missing filename, when it adds something. */
export function attachmentName(a: Attachment): string | null {
  return named(a.file_name) ?? (a.is_image ? null : named(a.content_type));
}

/** Noun and name together: `image`, `image: shot.png`, `attachment: audio/ogg`.
 *  For somewhere with no icon to lean on. */
export function attachmentLabel(a: Attachment): string {
  const name = attachmentName(a);
  const noun = attachmentNoun(a);
  return name != null ? `${noun}: ${name}` : noun;
}
