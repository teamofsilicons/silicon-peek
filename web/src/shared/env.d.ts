/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Space Station table *name* for automatic analytics (never a key). */
  readonly VITE_PEEK_ANALYTICS_TABLE?: string;
  /** Space Station table *name* for explicit events (never a key). */
  readonly VITE_PEEK_EVENTS_TABLE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
