// Shapes returned by the Rust backend (serde → snake_case), plus /api/me.

export type Origin = 'signal' | 'gchat';

export interface Me {
  user_id: string;
  display_name: string;
}

export interface Conversation {
  origin: Origin;
  id: string;
  name: string | null;
  kind: 'dm' | 'group';
  message_count: number;
  last_ts: number | null; // ms epoch
}

export interface Reaction {
  emoji: string;
  count: number;
}

export interface Attachment {
  id: string;
  content_type: string | null;
  file_name: string | null;
  size: number | null;
  available: boolean;
  is_image: boolean;
}

export interface Message {
  id: string;
  ts: number; // ms epoch
  sender: string;
  is_outgoing: boolean;
  body: string | null;
  deleted: boolean;
  edited: boolean;
  reactions: Reaction[];
  attachments: Attachment[];
}

export interface MessagesPage {
  messages: Message[]; // ascending by ts
  has_more: boolean;
  next_cursor: string | null; // opaque; pass back as ?cursor to page older
}

export interface SearchHit {
  origin: Origin;
  conversation_id: string;
  conversation_name: string | null;
  ts: number;
  sender: string;
  snippet: string;
}

/** One thing that happened in the client, POSTed to /api/telemetry to be logged.
 *  `kind` is 'nav' (a route change, no label) or 'tap' (a control, `label` its
 *  visible text verbatim). `at` is the client's clock in epoch ms — a batch
 *  arrives at once, so the server's receive time cannot order events within it. */
export interface TelemetryEvent {
  kind: string;
  path: string;
  label: string | null;
  at: number;
}
