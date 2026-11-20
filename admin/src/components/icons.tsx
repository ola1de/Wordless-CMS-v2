import type { JSX } from "react";

/**
 * Wordless icon audit (P4, 2026-11-19) — 104 icons total.
 *
 * 72 icons ported from the v2 UI preview artifact
 * (~/workspace/ts-spaces/wordless-v2-ui-preview/index.html, ICON_LIB):
 *   - Content & editing: 12 (Home, File text, Folder, Pen, Type / Heading, List, Quote, Table, Code, Link, Layout, Layers)
 *   - Media: 10 (Image, Video, Music, Microphone, Camera, Film, Headphones, Upload, Download, Play)
 *   - Commerce: 10 (Cart, Shopping bag, Price tag, Card, Percent, Delivery truck, Package, Receipt, Store, Banknote)
 *   - People & community: 12 (Users, Add user, Heart, Star, Message, Bell, Share, Thumbs up, Smile, Send, Bookmark, Mail)
 *   - School & learning: 6 (Graduation cap, Book, Award, Clipboard check, Target, Timer)
 *   - System & tools: 22 (Search, Settings gear, Shield, Lock, Eye, Trash, Plus, Check, Close, Chevron down, Arrow right, Refresh, Filter, Calendar, Clock, Globe, Zap, Sparkles · AI, Chart, Database, Key, Palette)
 *
 * 31 v1-only icons kept (still referenced by existing pages, not in
 * the preview 72) and ported in the same style, plus `x` kept as an
 * alias of preview Close (identical path data). The 31: dashboard,
 * posts, pages, media, terms, comments, menus, health, activity,
 * conversion, themes, ai, seo, studio, commerce, integrations,
 * security, backup, import, edit, checkCircle, alert, info,
 * chevronLeft, chevronRight, logout, external, user, file, settings.
 *
 * Where a v1 name and a preview name coincide (search, eye, trash,
 * plus, check, chevronDown, lock, clock, globe, zap, link, upload,
 * download, image, users, sparkles) the preview path data is used.
 *
 * P4 bar: the preview's 72 icons. The 150+ icon count in doc §6
 * principle 6 is a [target], not the P4 bar — 150+ remains open.
 * Style: 24px grid, one stroke, currentColor, dark-mode aware.
 * The Icon component keeps strokeWidth 2 (the existing v1 pattern);
 * the preview draws at 1.8 — the path data is identical, the weight
 * is the admin's existing one. No emoji, no icon font, no external CDN.
 */

export type IconName =
  | "home"
  | "fileText"
  | "folder"
  | "pen"
  | "typeHeading"
  | "list"
  | "quote"
  | "table"
  | "code"
  | "link"
  | "layout"
  | "layers"
  | "image"
  | "video"
  | "music"
  | "microphone"
  | "camera"
  | "film"
  | "headphones"
  | "upload"
  | "download"
  | "play"
  | "cart"
  | "shoppingBag"
  | "priceTag"
  | "card"
  | "percent"
  | "deliveryTruck"
  | "package"
  | "receipt"
  | "store"
  | "banknote"
  | "users"
  | "addUser"
  | "heart"
  | "star"
  | "message"
  | "bell"
  | "share"
  | "thumbsUp"
  | "smile"
  | "send"
  | "bookmark"
  | "mail"
  | "graduationCap"
  | "book"
  | "award"
  | "clipboardCheck"
  | "target"
  | "timer"
  | "search"
  | "settingsGear"
  | "shield"
  | "lock"
  | "eye"
  | "trash"
  | "plus"
  | "check"
  | "close"
  | "chevronDown"
  | "arrowRight"
  | "refresh"
  | "filter"
  | "calendar"
  | "clock"
  | "globe"
  | "zap"
  | "sparkles"
  | "chart"
  | "database"
  | "key"
  | "palette"
  | "activity"
  | "ai"
  | "alert"
  | "backup"
  | "checkCircle"
  | "chevronLeft"
  | "chevronRight"
  | "comments"
  | "commerce"
  | "conversion"
  | "dashboard"
  | "edit"
  | "external"
  | "file"
  | "health"
  | "import"
  | "info"
  | "integrations"
  | "logout"
  | "media"
  | "menu"
  | "menus"
  | "pages"
  | "posts"
  | "security"
  | "seo"
  | "settings"
  | "studio"
  | "terms"
  | "themes"
  | "user"
  | "x";

const PATHS: Record<IconName, JSX.Element> = {
  // Content & editing
  home: (
    <>
      <path d="m3 11 9-8 9 8"/>
      <path d="M5 10v10h14V10"/>
    </>
  ),
  fileText: (
    <>
      <path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9Z"/>
      <path d="M14 3v6h6M9 13h6M9 17h6"/>
    </>
  ),
  folder: <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>,
  pen: (
    <>
      <path d="M12 20h9"/>
      <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/>
    </>
  ),
  typeHeading: <path d="M5 7V5h14v2M12 5v14M9 19h6"/>,
  list: (
    <>
      <path d="M9 6h11M9 12h11M9 18h11"/>
      <circle cx="4.5" cy="6" r="1" fill="currentColor"/>
      <circle cx="4.5" cy="12" r="1" fill="currentColor"/>
      <circle cx="4.5" cy="18" r="1" fill="currentColor"/>
    </>
  ),
  quote: <path d="M10 8c-3 1-4 3-4 6v2h5v-6H8c.3-1 1-1.6 2-2ZM20 8c-3 1-4 3-4 6v2h5v-6h-3c.3-1 1-1.6 2-2Z"/>,
  table: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2"/>
      <path d="M3 10h18M3 15h18M9 10v10M15 10v10"/>
    </>
  ),
  code: <path d="m8 6-6 6 6 6M16 6l6 6-6 6"/>,
  link: (
    <>
      <path d="M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1.7 1.7"/>
      <path d="M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1.7-1.7"/>
    </>
  ),
  layout: (
    <>
      <rect x="3" y="3" width="18" height="18" rx="2"/>
      <path d="M3 9h18M9 21V9"/>
    </>
  ),
  layers: (
    <>
      <path d="m12 3 9 5-9 5-9-5Z"/>
      <path d="m3 13 9 5 9-5"/>
    </>
  ),
  // Media
  image: (
    <>
      <rect x="3" y="3" width="18" height="18" rx="2"/>
      <circle cx="9" cy="9" r="2"/>
      <path d="m21 15-4.5-4.5L6 21"/>
    </>
  ),
  video: (
    <>
      <rect x="2" y="6" width="13" height="12" rx="2"/>
      <path d="m15 10 6-3v10l-6-3"/>
    </>
  ),
  music: (
    <>
      <path d="M9 18V6l10-2v11"/>
      <circle cx="6.5" cy="18" r="2.5"/>
      <circle cx="16.5" cy="15" r="2.5"/>
    </>
  ),
  microphone: (
    <>
      <rect x="9" y="3" width="6" height="11" rx="3"/>
      <path d="M5 11a7 7 0 0 0 14 0M12 18v3"/>
    </>
  ),
  camera: (
    <>
      <path d="M4 8h3l2-3h6l2 3h3a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1Z"/>
      <circle cx="12" cy="14" r="3.5"/>
    </>
  ),
  film: (
    <>
      <rect x="3" y="3" width="18" height="18" rx="2"/>
      <path d="M8 3v18M16 3v18M3 8h5M3 16h5M16 8h5M16 16h5"/>
    </>
  ),
  headphones: (
    <>
      <path d="M4 15v-2a8 8 0 0 1 16 0v2"/>
      <rect x="3" y="14" width="4" height="6" rx="1.5"/>
      <rect x="17" y="14" width="4" height="6" rx="1.5"/>
    </>
  ),
  upload: (
    <>
      <path d="M12 16V4M7 9l5-5 5 5"/>
      <path d="M4 20h16"/>
    </>
  ),
  download: (
    <>
      <path d="M12 4v12M7 11l5 5 5-5"/>
      <path d="M4 20h16"/>
    </>
  ),
  play: (
    <>
      <circle cx="12" cy="12" r="9"/>
      <path d="m10 8.5 6 3.5-6 3.5Z" fill="currentColor" stroke="none"/>
    </>
  ),
  // Commerce
  cart: (
    <>
      <circle cx="9" cy="20" r="1.6"/>
      <circle cx="17" cy="20" r="1.6"/>
      <path d="M2 3h3l2.6 12.5a1.5 1.5 0 0 0 1.5 1.2h7.9a1.5 1.5 0 0 0 1.5-1.2L20.5 7H6"/>
    </>
  ),
  shoppingBag: (
    <>
      <path d="M5 8h14l-1 12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1Z"/>
      <path d="M8.5 10V6a3.5 3.5 0 0 1 7 0v4"/>
    </>
  ),
  priceTag: (
    <>
      <path d="M12 3H4v8l9 9a1.5 1.5 0 0 0 2 0l6-6a1.5 1.5 0 0 0 0-2Z"/>
      <circle cx="8.5" cy="7.5" r="1.3" fill="currentColor"/>
    </>
  ),
  card: (
    <>
      <rect x="2" y="5" width="20" height="14" rx="2"/>
      <path d="M2 10h20"/>
    </>
  ),
  percent: (
    <>
      <path d="M19 5 5 19"/>
      <circle cx="7" cy="7" r="2.5"/>
      <circle cx="17" cy="17" r="2.5"/>
    </>
  ),
  deliveryTruck: (
    <>
      <path d="M2 6h12v10H2zM14 10h4l3 3v3h-7"/>
      <circle cx="6.5" cy="18" r="1.8"/>
      <circle cx="17" cy="18" r="1.8"/>
    </>
  ),
  package: (
    <>
      <path d="m12 3 8 4.5v9L12 21l-8-4.5v-9Z"/>
      <path d="M4 7.5 12 12l8-4.5M12 12v9"/>
    </>
  ),
  receipt: (
    <>
      <path d="M5 3h14v18l-2.3-1.5L14.4 21l-2.4-1.5L9.6 21l-2.3-1.5L5 21Z"/>
      <path d="M9 8h6M9 12h6"/>
    </>
  ),
  store: (
    <>
      <path d="M4 7 5.5 3h13L20 7a3 3 0 0 1-6 0 3 3 0 0 1-6 0 3 3 0 0 1-6 0Z" transform="translate(0,1)"/>
      <path d="M5 12v8h14v-8"/>
    </>
  ),
  banknote: (
    <>
      <rect x="2" y="6" width="20" height="12" rx="2"/>
      <circle cx="12" cy="12" r="3"/>
      <path d="M5.5 9.5h.01M18.5 14.5h.01"/>
    </>
  ),
  // People & community
  users: (
    <>
      <circle cx="9" cy="8" r="3.5"/>
      <path d="M2.5 20c.8-3.2 3.4-5 6.5-5s5.7 1.8 6.5 5"/>
      <path d="M16 5a3.5 3.5 0 0 1 0 6.6M17.5 15.4c2 .6 3.4 2.1 4 4.6"/>
    </>
  ),
  addUser: (
    <>
      <circle cx="10" cy="8" r="3.5"/>
      <path d="M3.5 20c.8-3.2 3.4-5 6.5-5 1.2 0 2.3.3 3.3.8M18 8v6M15 11h6"/>
    </>
  ),
  heart: <path d="M12 20.5C6 16.5 3 13.3 3 9.6A4.6 4.6 0 0 1 7.6 5c1.7 0 3.4 1 4.4 2.4A5.4 5.4 0 0 1 16.4 5 4.6 4.6 0 0 1 21 9.6c0 3.7-3 6.9-9 10.9Z"/>,
  star: <path d="m12 3.5 2.6 5.3 5.9.9-4.3 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8L3.5 9.7l5.9-.9Z"/>,
  message: <path d="M21 11.5a8.4 8.4 0 0 1-8.5 8.3c-1.5 0-3-.4-4.2-1.1L3 20l1.4-5.1A8.3 8.3 0 0 1 3.5 11 8.4 8.4 0 0 1 12 3a8.4 8.4 0 0 1 9 8.5Z"/>,
  bell: (
    <>
      <path d="M6 9a6 6 0 0 1 12 0c0 5 2 6 2 6H4s2-1 2-6"/>
      <path d="M10 20a2 2 0 0 0 4 0"/>
    </>
  ),
  share: (
    <>
      <circle cx="6" cy="12" r="2.5"/>
      <circle cx="17" cy="5.5" r="2.5"/>
      <circle cx="17" cy="18.5" r="2.5"/>
      <path d="m8.2 10.8 6.6-4M8.2 13.2l6.6 4"/>
    </>
  ),
  thumbsUp: (
    <>
      <path d="M7 11v10H4a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1Z"/>
      <path d="M7 11 12.5 3c1.8 0 2.8 1.3 2.5 3l-.7 3.5H20a2 2 0 0 1 2 2.4l-1.2 6.6a2 2 0 0 1-2 1.5H7"/>
    </>
  ),
  smile: (
    <>
      <circle cx="12" cy="12" r="9"/>
      <path d="M8.5 14.5a5 5 0 0 0 7 0"/>
      <path d="M9 9.5h.01M15 9.5h.01"/>
    </>
  ),
  send: <path d="m21 3-9.5 9.5M21 3l-6.8 18-2.7-8.5L3 9.7Z"/>,
  bookmark: <path d="M6 3h12v18l-6-4-6 4Z"/>,
  mail: (
    <>
      <rect x="2" y="5" width="20" height="14" rx="2"/>
      <path d="m3 7 9 6 9-6"/>
    </>
  ),
  // School & learning
  graduationCap: (
    <>
      <path d="m12 4 10 5-10 5L2 9Z"/>
      <path d="M6 11.5V16c0 1.7 2.7 3 6 3s6-1.3 6-3v-4.5M22 9v5"/>
    </>
  ),
  book: (
    <>
      <path d="M5 3h13a1 1 0 0 1 1 1v16H5a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1Z"/>
      <path d="M4 19a1 1 0 0 1 1-1h14"/>
    </>
  ),
  award: (
    <>
      <circle cx="12" cy="9" r="6"/>
      <path d="m8.5 14-2 7 5.5-3 5.5 3-2-7"/>
    </>
  ),
  clipboardCheck: (
    <>
      <rect x="5" y="4" width="14" height="17" rx="2"/>
      <path d="M9 4a3 3 0 0 1 6 0M9.5 14l2 2 3.5-4"/>
    </>
  ),
  target: (
    <>
      <circle cx="12" cy="12" r="9"/>
      <circle cx="12" cy="12" r="5"/>
      <circle cx="12" cy="12" r="1.2" fill="currentColor"/>
    </>
  ),
  timer: (
    <>
      <circle cx="12" cy="13" r="8"/>
      <path d="M12 9v4l2.5 2M9 2.5h6"/>
    </>
  ),
  // System & tools
  search: (
    <>
      <circle cx="11" cy="11" r="7"/>
      <path d="m21 21-4.3-4.3"/>
    </>
  ),
  settingsGear: (
    <>
      <circle cx="12" cy="12" r="3.2"/>
      <path d="M12 2.8v3M12 18.2v3M2.8 12h3M18.2 12h3M5.5 5.5l2.1 2.1M16.4 16.4l2.1 2.1M18.5 5.5l-2.1 2.1M7.6 16.4l-2.1 2.1"/>
    </>
  ),
  shield: (
    <>
      <path d="M12 3 5 5.8v5.4c0 4.5 3 7.7 7 9.3 4-1.6 7-4.8 7-9.3V5.8Z"/>
      <path d="m9 12 2 2 4-4"/>
    </>
  ),
  lock: (
    <>
      <rect x="4" y="11" width="16" height="10" rx="2"/>
      <path d="M8 11V7a4 4 0 0 1 8 0v4"/>
    </>
  ),
  eye: (
    <>
      <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z"/>
      <circle cx="12" cy="12" r="3"/>
    </>
  ),
  trash: <path d="M4 7h16M9 7V4h6v3M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13"/>,
  plus: <path d="M12 5v14M5 12h14"/>,
  check: <path d="m4 12.5 5 5L20 6.5"/>,
  close: <path d="M6 6l12 12M18 6 6 18"/>,
  chevronDown: <path d="m6 9 6 6 6-6"/>,
  arrowRight: <path d="M4 12h16M14 6l6 6-6 6"/>,
  refresh: <path d="M20 12a8 8 0 1 1-2.3-5.6M20 3v4h-4"/>,
  filter: <path d="M3 5h18l-7 8v5l-4 2v-7Z"/>,
  calendar: (
    <>
      <rect x="3" y="5" width="18" height="16" rx="2"/>
      <path d="M3 10h18M8 3v4M16 3v4"/>
    </>
  ),
  clock: (
    <>
      <circle cx="12" cy="12" r="9"/>
      <path d="M12 7v5l3.5 2"/>
    </>
  ),
  globe: (
    <>
      <circle cx="12" cy="12" r="9"/>
      <path d="M3 12h18M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18"/>
    </>
  ),
  zap: <path d="M13 2 4 14h6l-1 8 9-12h-6Z"/>,
  sparkles: (
    <>
      <path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8Z"/>
      <path d="M19 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8Z"/>
    </>
  ),
  chart: <path d="M4 20V10M10 20V4M16 20v-8M21 20H3"/>,
  database: (
    <>
      <ellipse cx="12" cy="5.5" rx="8" ry="3"/>
      <path d="M4 5.5v13c0 1.7 3.6 3 8 3s8-1.3 8-3v-13M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>
    </>
  ),
  key: (
    <>
      <circle cx="8" cy="15" r="4.5"/>
      <path d="m11.2 11.8 9-9M17 4l3 3M14.5 6.5l2.5 2.5"/>
    </>
  ),
  palette: (
    <>
      <circle cx="12" cy="12" r="9"/>
      <circle cx="8.5" cy="10" r="1.2" fill="currentColor"/>
      <circle cx="12" cy="7.5" r="1.2" fill="currentColor"/>
      <circle cx="15.5" cy="10" r="1.2" fill="currentColor"/>
      <path d="M12 21a2.5 2.5 0 0 1 0-5h2.5A3.5 3.5 0 0 0 18 12.5"/>
    </>
  ),
  // Kept from v1 (still referenced by existing pages, not in the preview 72)
  activity: (
    <>
      <path d="M3 12a9 9 0 1 0 3-6.7" />
      <path d="M3 4v5h5" />
      <path d="M12 7v5l3 2" />
    </>
  ),
  ai: (
    <>
      <path d="M12 3l1.8 4.7 4.7 1.8-4.7 1.8L12 16l-1.8-4.7L5.5 9.5l4.7-1.8L12 3z" />
      <path d="M19 15l.8 2.2 2.2.8-2.2.8L19 21l-.8-2.2-2.2-.8 2.2-.8L19 15z" />
    </>
  ),
  alert: (
    <>
      <path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
      <path d="M12 9v4" />
      <path d="M12 17h.01" />
    </>
  ),
  backup: (
    <>
      <ellipse cx="12" cy="5" rx="8" ry="3" />
      <path d="M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5" />
      <path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3" />
    </>
  ),
  checkCircle: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M8.5 12.5l2.5 2.5 4.5-5" />
    </>
  ),
  chevronLeft: <path d="M15 18l-6-6 6-6" />,
  chevronRight: <path d="M9 18l6-6-6-6" />,
  comments: <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />,
  commerce: (
    <>
      <path d="M6 7h12l1.2 13a2 2 0 0 1-2 2.2H6.8a2 2 0 0 1-2-2.2L6 7z" />
      <path d="M9 10V6a3 3 0 0 1 6 0v4" />
    </>
  ),
  conversion: (
    <>
      <path d="M21 12a9 9 0 1 1-2.6-6.4" />
      <path d="M21 3v6h-6" />
    </>
  ),
  dashboard: (
    <>
      <rect x="3" y="3" width="7" height="7" rx="1.5" />
      <rect x="14" y="3" width="7" height="7" rx="1.5" />
      <rect x="3" y="14" width="7" height="7" rx="1.5" />
      <rect x="14" y="14" width="7" height="7" rx="1.5" />
    </>
  ),
  edit: <path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" />,
  external: (
    <>
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <path d="M15 3h6v6" />
      <path d="M10 14 21 3" />
    </>
  ),
  file: (
    <>
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <path d="M14 2v6h6" />
    </>
  ),
  health: <path d="M3 12h4l3 8 4-16 3 8h4" />,
  import: (
    <>
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <path d="M17 8l-5-5-5 5" />
      <path d="M12 3v12" />
    </>
  ),
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5" />
      <path d="M12 8h.01" />
    </>
  ),
  integrations: (
    <>
      <path d="M9 7V3" />
      <path d="M15 7V3" />
      <path d="M7 7h10v4a5 5 0 0 1-10 0V7z" />
      <path d="M12 16v5" />
    </>
  ),
  logout: (
    <>
      <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
      <path d="M16 17l5-5-5-5" />
      <path d="M21 12H9" />
    </>
  ),
  media: (
    <>
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <circle cx="9" cy="9" r="2" />
      <path d="M21 15l-5-5-9 9" />
    </>
  ),
  menu: (
    <>
      <path d="M4 6h16" />
      <path d="M4 12h16" />
      <path d="M4 18h16" />
    </>
  ),
  menus: (
    <>
      <path d="M8 6h13" />
      <path d="M8 12h13" />
      <path d="M8 18h13" />
      <path d="M3.5 6h.01" />
      <path d="M3.5 12h.01" />
      <path d="M3.5 18h.01" />
    </>
  ),
  pages: (
    <>
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <path d="M14 2v6h6" />
    </>
  ),
  posts: (
    <>
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <path d="M14 2v6h6" />
      <path d="M9 13h6" />
      <path d="M9 17h6" />
    </>
  ),
  security: (
    <>
      <path d="M12 3l7 3v5c0 4.5-3 8.5-7 10-4-1.5-7-5.5-7-10V6l7-3z" />
      <path d="M9 12l2 2 4-4" />
    </>
  ),
  seo: (
    <>
      <circle cx="11" cy="11" r="7" />
      <path d="M20 20l-3.5-3.5" />
    </>
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3.2" />
      <path d="M12 2.7v2.5" />
      <path d="M12 18.8v2.5" />
      <path d="M2.7 12h2.5" />
      <path d="M18.8 12h2.5" />
      <path d="M5.4 5.4l1.8 1.8" />
      <path d="M16.8 16.8l1.8 1.8" />
      <path d="M18.6 5.4l-1.8 1.8" />
      <path d="M7.2 16.8l-1.8 1.8" />
    </>
  ),
  studio: (
    <>
      <rect x="3" y="5" width="18" height="16" rx="2" />
      <path d="M3 10h18" />
      <path d="M8 3v4" />
      <path d="M16 3v4" />
    </>
  ),
  terms: (
    <>
      <path d="M20.6 13.4 11 3.8A2 2 0 0 0 9.6 3.2H4a1 1 0 0 0-1 1v5.6c0 .5.2 1 .6 1.4l9.6 9.6a2 2 0 0 0 2.8 0l4.6-4.6a2 2 0 0 0 0-2.8z" />
      <circle cx="7.5" cy="7.5" r="1.5" />
    </>
  ),
  themes: (
    <>
      <circle cx="12" cy="12" r="9" />
      <circle cx="8.5" cy="10.5" r="1.1" fill="currentColor" stroke="none" />
      <circle cx="12" cy="7.5" r="1.1" fill="currentColor" stroke="none" />
      <circle cx="15.5" cy="10.5" r="1.1" fill="currentColor" stroke="none" />
      <path d="M12 21a1.6 1.6 0 0 0 1.6-1.6c0-.9-.6-1.3-1.1-1.8-.5-.4-.9-.9-.9-1.6" />
    </>
  ),
  user: (
    <>
      <circle cx="12" cy="8" r="4" />
      <path d="M4 21v-1a8 8 0 0 1 16 0v1" />
    </>
  ),
  x: <path d="M6 6l12 12M18 6 6 18"/>,
};

export function Icon({
  name,
  size = 18,
  className,
}: {
  name: IconName;
  size?: number;
  className?: string;
}): JSX.Element {
  return (
    <svg
      className={className ?? "icon"}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {PATHS[name]}
    </svg>
  );
}

/**
 * Wordless wordmark — rounded teal tile with a white "W" stroke.
 * Used on the login page and in the sidebar brand.
 */
export function LogoMark({ size = 40 }: { size?: number }): JSX.Element {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 40 40"
      aria-hidden="true"
      focusable="false"
      role="img"
    >
      <defs>
        <linearGradient id="wl-logo-g" x1="0" y1="0" x2="40" y2="40">
          <stop offset="0" stopColor="#14b8a6" />
          <stop offset="1" stopColor="#0b7c74" />
        </linearGradient>
      </defs>
      <rect width="40" height="40" rx="11" fill="url(#wl-logo-g)" />
      <path
        d="M9 12 L14 28 L20 15 L26 28 L31 12"
        fill="none"
        stroke="#ffffff"
        strokeWidth="3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
