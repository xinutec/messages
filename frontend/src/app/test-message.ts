// A message for specs, with every field at its quietest; a spec overrides what
// it exercises. One place, so a new field on `Message` is one edit.

import { Message } from './models';

export function testMessage(over: Partial<Message> = {}): Message {
  return {
    id: '1',
    ts: 0,
    sender: 's',
    is_outgoing: false,
    kind: 'message',
    body: 'b',
    deleted: false,
    edited: false,
    reactions: [],
    attachments: [],
    link_images: [],
    link_offers: [],
    edits: [],
    reply_to: null,
    delivery: null,
    entities: [],
    album: null,
    previews: [],
    ...over,
  };
}
