// What to call an attachment.
//
// ⚠ **TWO PLACES PRINT THIS AND THEY MUST AGREE.** The thread renders it
// (`thread.html`) and `copy-log.ts` writes it to the clipboard. While each
// composed its own label they drifted, and the drift was invisible from either
// side alone: a stored-less image with no filename read `image/jpeg (not
// stored)` on screen and `[image (not stored)]` in the paste. The decision is
// here so there is only one of it.
//
// The two callers still SHAPE it differently, and should: on screen an icon
// already says what kind of thing it is, so the template prints the name alone;
// a pasted log has no icon, so it prints the noun too.

import { Attachment } from './models';

/** An EMPTY name is as good as a missing one — `image: ` names nothing. Spelt
 *  out rather than left to `||` because `??` would keep the empty string, and
 *  the difference between them is invisible until it happens. */
const named = (s: string | null): string | null => (s != null && s !== '' ? s : null);

/** What kind of thing it is — and so what to call it when it has no name. */
export function attachmentNoun(a: Attachment): string {
  return a.is_image ? 'image' : 'attachment';
}

/** What to call it, or null when nothing to hand says more than the noun does.
 *
 *  ⚠ The content type stands in for a MISSING FILENAME, and only when it adds
 *  something. A nameless jpeg printed `[image: image/jpeg]` — the word twice —
 *  and it reached a real paste before anyone noticed, because on screen that
 *  branch does not exist: the template renders the picture, and falls back to
 *  the type only for a file it cannot show. */
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
