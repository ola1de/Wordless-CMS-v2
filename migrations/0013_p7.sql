-- WORDLESS v2 P7 — social networking (BuddyPress-like).
-- Q11 decided 2026-11-20: friends (mutual, request/accept) is THE
-- relationship model; follows are NOT built in P7, so there is NO
-- wl_follows table here — a future phase can add it without touching
-- the friends model.
--
-- Blocking lives on wl_friendships with status='blocked': the row
-- (requester, addressee, 'blocked') means requester blocked addressee.
-- Either direction blocks both ways for visibility and messaging.
-- A blocked row is never also a friendship; block upserts over any
-- pending/accepted row in either direction.
CREATE TABLE wl_social_profiles (
  user_id INTEGER PRIMARY KEY,
  display_name TEXT NOT NULL DEFAULT '',
  bio TEXT NOT NULL DEFAULT '',
  avatar_id INTEGER,
  cover_id INTEGER,
  fields_json TEXT NOT NULL DEFAULT '{}',
  visibility TEXT NOT NULL DEFAULT 'public' CHECK (visibility IN ('public', 'friends', 'private')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_friendships (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  requester_id INTEGER NOT NULL,
  addressee_id INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'blocked')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (requester_id, addressee_id)
);
CREATE INDEX idx_wl_friendships_pair ON wl_friendships (requester_id, addressee_id, status);
CREATE INDEX idx_wl_friendships_addressee ON wl_friendships (addressee_id, status);
CREATE TABLE wl_activity_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  actor_id INTEGER NOT NULL,
  type TEXT NOT NULL DEFAULT 'post' CHECK (type IN ('post', 'comment', 'like', 'join', 'mention')),
  object_type TEXT NOT NULL DEFAULT '',
  object_id INTEGER NOT NULL DEFAULT 0,
  content TEXT NOT NULL DEFAULT '',
  visibility TEXT NOT NULL DEFAULT 'public' CHECK (visibility IN ('public', 'friends', 'group', 'private')),
  group_id INTEGER,
  status TEXT NOT NULL DEFAULT 'visible' CHECK (status IN ('visible', 'hidden', 'deleted')),
  hidden_by INTEGER,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_activity_actor ON wl_activity_items (actor_id, created_at DESC);
CREATE INDEX idx_wl_activity_feed ON wl_activity_items (created_at DESC, id DESC);
CREATE INDEX idx_wl_activity_group ON wl_activity_items (group_id, created_at DESC);
CREATE TABLE wl_activity_likes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE (item_id, user_id)
);
CREATE INDEX idx_wl_activity_likes_item ON wl_activity_likes (item_id);
CREATE TABLE wl_groups (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  visibility TEXT NOT NULL DEFAULT 'public' CHECK (visibility IN ('public', 'private', 'hidden')),
  avatar_id INTEGER,
  created_by INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_wl_groups_visibility ON wl_groups (visibility);
CREATE TABLE wl_group_members (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  group_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'moderator', 'admin')),
  status TEXT NOT NULL DEFAULT 'member' CHECK (status IN ('member', 'requested', 'invited', 'banned')),
  created_at TEXT NOT NULL,
  UNIQUE (group_id, user_id)
);
CREATE INDEX idx_wl_group_members_group ON wl_group_members (group_id, status);
CREATE INDEX idx_wl_group_members_user ON wl_group_members (user_id, status);
CREATE TABLE wl_message_threads (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE wl_thread_participants (
  thread_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  last_read_at TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (thread_id, user_id)
);
CREATE INDEX idx_wl_thread_participants_user ON wl_thread_participants (user_id);
CREATE TABLE wl_messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  thread_id INTEGER NOT NULL,
  sender_id INTEGER NOT NULL,
  body TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
CREATE INDEX idx_wl_messages_thread ON wl_messages (thread_id, created_at);
CREATE TABLE wl_notifications (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER NOT NULL,
  type TEXT NOT NULL DEFAULT '',
  actor_id INTEGER NOT NULL DEFAULT 0,
  object_type TEXT NOT NULL DEFAULT '',
  object_id INTEGER NOT NULL DEFAULT 0,
  read_at TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_wl_notifications_user ON wl_notifications (user_id, read_at, created_at DESC);
