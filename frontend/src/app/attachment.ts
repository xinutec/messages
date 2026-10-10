// What to call an attachment, shared by the thread (thread.html) and the
// clipboard (copy-log.ts). The screen shows an icon, so it prints the name
// alone; a pasted log prints the noun too.

import { Attachment } from './models';

/** An empty name is no name; `??` would keep the empty string. */
const named = (s: string | null): string | null => (s != null && s !== '' ? s : null);

/** Whether it is a picture: the thread shows it, and opens it in the viewer. */
export function isImage(a: Attachment): boolean {
  return a.content_type?.startsWith('image/') ?? false;
}

/** Whether it plays: the thread draws it in place rather than as a file. */
export function isVideo(a: Attachment): boolean {
  return a.content_type?.startsWith('video/') ?? false;
}

/** What kind of thing it is, and so what to call it without a name. */
export function attachmentNoun(a: Attachment): string {
  return isImage(a) ? 'image' : isVideo(a) ? 'video' : 'attachment';
}

/** What to call it, or null when nothing says more than the noun. The content
 *  type stands in for a missing filename, when it adds something. */
export function attachmentName(a: Attachment): string | null {
  return named(a.file_name) ?? (isImage(a) || isVideo(a) ? null : named(a.content_type));
}

/** Noun and name together: `image`, `image: shot.png`, `attachment: audio/ogg`.
 *  For somewhere with no icon to lean on. */
export function attachmentLabel(a: Attachment): string {
  const name = attachmentName(a);
  const noun = attachmentNoun(a);
  return name != null ? `${noun}: ${name}` : noun;
}
