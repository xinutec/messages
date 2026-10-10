// Which rendered messages a selection touches, by their `data-id`: the DOM says
// which, the model says what. The clipboard's log is made from them
// (copy-log.ts). Only the rendered window can be selected.

import { Message } from './models';

export function selectedMessages(
  el: HTMLElement | undefined,
  sel: Selection | null,
  rendered: readonly Message[],
): Message[] {
  if (el == null || sel == null || sel.isCollapsed) return [];
  // Firefox allows several ranges.
  const ranges = Array.from({ length: sel.rangeCount }, (_, i) => sel.getRangeAt(i));
  const ids = new Set<string>();
  for (const node of el.querySelectorAll<HTMLElement>('.msg[data-id]')) {
    // `intersectsNode`, not `containsNode`: a selection inside one bubble
    // selects that message. jsdom gets both wrong; e2e/copy.spec.ts covers it.
    if (ranges.some((r) => r.intersectsNode(node))) {
      const id = node.dataset['id'];
      if (id != null) ids.add(id);
    }
  }
  return rendered.filter((m) => ids.has(m.id));
}
