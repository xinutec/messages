// Barrel over the ts-rs–generated wire types (frontend/src/app/generated/),
// regenerated from the Rust API types by scripts/gen-types.sh. Import from here.
//
// These were hand-written until 2026-07-30. They had not drifted, but nothing
// would have said so if they had — and the drift they were most likely to suffer
// was already there in reverse: `origin` and `kind` were string unions here while
// the Rust side carried plain `String`. The enums came first so generating from
// Rust would not throw that precision away.

export * from "./generated/Attachment";
export * from "./generated/Conversation";
export * from "./generated/ConversationKind";
export * from "./generated/Me";
export * from "./generated/Message";
export * from "./generated/MessageKind";
export * from "./generated/MessagesPage";
export * from "./generated/Origin";
export * from "./generated/Reaction";
export * from "./generated/SearchHit";
export * from "./generated/SendRequest";
export * from "./generated/SendResult";
export * from "./generated/TelemetryEvent";
