/**
 * Community (§6.3, P7) — the social network in the admin: member
 * directory, groups management, activity moderation (hide), and a
 * messaging overview. Member profiles are edited by the member
 * (their own profile page); this screen is for discovery and
 * moderation, not for editing other people's profiles.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatDateTime } from "../lib/format";
import type {
  SocialActivityItem,
  SocialGroup,
  SocialGroupList,
  SocialMemberList,
  SocialThread,
} from "../lib/types";
import {
  Alert,
  Badge,
  Button,
  Card,
  EmptyState,
  Input,
  Select,
  Skeleton,
  Table,
  Tabs,
} from "../components/elements";

type TabId = "members" | "groups" | "activity" | "messages";

export function SocialPage({ timezone }: { timezone: string }): React.ReactElement {
  const [tab, setTab] = useState<TabId>("members");
  const [error, setError] = useState<string | null>(null);
  return (
    <div>
      {error !== null ? (
        <Alert tone="err" title="Community" onDismiss={() => setError(null)}>
          {error}
        </Alert>
      ) : null}
      <Tabs
        ariaLabel="Community sections"
        value={tab}
        onChange={(id) => setTab(id as TabId)}
        tabs={[
          { id: "members", label: "Members" },
          { id: "groups", label: "Groups" },
          { id: "activity", label: "Activity moderation" },
          { id: "messages", label: "Messages" },
        ]}
      />
      {tab === "members" ? <MembersTab timezone={timezone} onError={setError} /> : null}
      {tab === "groups" ? <GroupsTab onError={setError} /> : null}
      {tab === "activity" ? <ActivityTab timezone={timezone} onError={setError} /> : null}
      {tab === "messages" ? <MessagesTab timezone={timezone} onError={setError} /> : null}
    </div>
  );
}

function MembersTab({
  timezone,
  onError,
}: {
  timezone: string;
  onError: (msg: string) => void;
}): React.ReactElement {
  const [list, setList] = useState<SocialMemberList | null>(null);
  const [q, setQ] = useState("");
  const [page, setPage] = useState(1);

  const load = useCallback(async () => {
    try {
      setList(
        await api.get<SocialMemberList>(
          `/api/social/members?q=${encodeURIComponent(q)}&page=${page}&per_page=20`,
        ),
      );
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load members.");
    }
  }, [q, page, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <Card title="Member directory" sub={list !== null ? `${list.total} members` : undefined}>
      <div className="toolbar">
        <Input
          label="Search members"
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            setPage(1);
          }}
          placeholder="Name, login, or bio…"
        />
      </div>
      {list === null ? (
        <Skeleton height={180} label="Loading members" />
      ) : list.items.length === 0 ? (
        <EmptyState
          icon="users"
          title="No members found"
          description="Try a different search, or wait for members to join."
        />
      ) : (
        <>
          <Table
            caption="Member directory"
            columns={[
              { key: "member", label: "Member" },
              { key: "visibility", label: "Profile" },
              { key: "friend", label: "Friendship" },
              { key: "joined", label: "Joined" },
            ]}
            rows={list.items.map((m) => ({
              member: (
                <span>
                  <span className="row-title">{m.displayName}</span>
                  <span className="row-sub">@{m.login}</span>
                  {m.bio ? <span className="row-sub">{m.bio}</span> : null}
                </span>
              ),
              visibility: <Badge tone={m.visibility === "public" ? "teal" : "amber"}>{m.visibility}</Badge>,
              friend: m.friendStatus === "none" ? "—" : m.friendStatus.replace("_", " "),
              joined: formatDateTime(m.registered, timezone),
            }))}
          />
          <div className="pagination">
            <Button size="small" icon="chevronLeft" disabled={list.page <= 1} onClick={() => setPage(list.page - 1)}>
              Previous
            </Button>
            <span>
              Page {list.page} of {Math.max(list.totalPages, 1)} ({list.total} total)
            </span>
            <Button
              size="small"
              icon="chevronRight"
              disabled={list.page >= list.totalPages}
              onClick={() => setPage(list.page + 1)}
            >
              Next
            </Button>
          </div>
        </>
      )}
    </Card>
  );
}

function GroupsTab({ onError }: { onError: (msg: string) => void }): React.ReactElement {
  const [list, setList] = useState<SocialGroupList | null>(null);
  const [page, setPage] = useState(1);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [visibility, setVisibility] = useState("public");
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    try {
      setList(await api.get<SocialGroupList>(`/api/social/groups?page=${page}&per_page=20`));
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load groups.");
    }
  }, [page, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  const create = async (): Promise<void> => {
    if (name.trim() === "") return;
    setCreating(true);
    try {
      await api.post<SocialGroup>("/api/social/groups", { name, description, visibility });
      setName("");
      setDescription("");
      setVisibility("public");
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to create group.");
    } finally {
      setCreating(false);
    }
  };

  const joinOrLeave = async (g: SocialGroup): Promise<void> => {
    try {
      if (g.myStatus === "member") {
        await api.post(`/api/social/groups/${g.id}/leave`, {});
      } else {
        await api.post(`/api/social/groups/${g.id}/join`, {});
      }
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to update membership.");
    }
  };

  return (
    <>
      <Card title="Create a group" sub="Public groups are open to everyone; private groups need approval; hidden groups are invite-only.">
        <div className="form-grid">
          <Input label="Group name" value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Lagos Writers" />
          <Input label="Description" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What is this group for?" />
          <Select
            label="Visibility"
            value={visibility}
            onChange={(e) => setVisibility(e.target.value)}
          >
            <option value="public">Public</option>
            <option value="private">Private</option>
            <option value="hidden">Hidden</option>
          </Select>
        </div>
        <Button disabled={creating || name.trim() === ""} onClick={() => void create()}>
          Create group
        </Button>
      </Card>
      <Card title="Groups" sub={list !== null ? `${list.total} groups` : undefined}>
        {list === null ? (
          <Skeleton height={180} label="Loading groups" />
        ) : list.items.length === 0 ? (
          <EmptyState
            icon="users"
            title="No groups yet"
            description="Create the first group above."
          />
        ) : (
          <>
            <Table
              caption="Groups"
              columns={[
                { key: "group", label: "Group" },
                { key: "visibility", label: "Visibility" },
                { key: "members", label: "Members" },
                { key: "membership", label: "Your membership" },
                { key: "action", label: "" },
              ]}
              rows={list.items.map((g) => ({
                group: (
                  <span>
                    <span className="row-title">{g.name}</span>
                    <span className="row-sub">/{g.slug}</span>
                    {g.description ? <span className="row-sub">{g.description}</span> : null}
                  </span>
                ),
                visibility: <Badge tone={g.visibility === "public" ? "teal" : "amber"}>{g.visibility}</Badge>,
                members: String(g.memberCount),
                membership:
                  g.myStatus === "none"
                    ? "—"
                    : g.myRole
                      ? `${g.myStatus} (${g.myRole})`
                      : g.myStatus,
                action: (
                  <Button size="small" onClick={() => void joinOrLeave(g)}>
                    {g.myStatus === "member" ? "Leave" : g.myStatus === "requested" ? "Requested" : "Join"}
                  </Button>
                ),
              }))}
            />
            <div className="pagination">
              <Button size="small" icon="chevronLeft" disabled={list.page <= 1} onClick={() => setPage(list.page - 1)}>
                Previous
              </Button>
              <span>
                Page {list.page} of {Math.max(list.totalPages, 1)} ({list.total} total)
              </span>
              <Button
                size="small"
                icon="chevronRight"
                disabled={list.page >= list.totalPages}
                onClick={() => setPage(list.page + 1)}
              >
                Next
              </Button>
            </div>
          </>
        )}
      </Card>
    </>
  );
}

function ActivityTab({
  timezone,
  onError,
}: {
  timezone: string;
  onError: (msg: string) => void;
}): React.ReactElement {
  const [items, setItems] = useState<SocialActivityItem[] | null>(null);
  const [page, setPage] = useState(1);

  const load = useCallback(async () => {
    try {
      setItems(await api.get<SocialActivityItem[]>(`/api/social/admin/activity?page=${page}&per_page=20`));
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load activity.");
    }
  }, [page, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  const hide = async (id: number): Promise<void> => {
    try {
      await api.post(`/api/social/activity/${id}/hide`, {});
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to hide activity.");
    }
  };

  return (
    <Card title="Activity moderation" sub="Hiding an item removes it from every feed, profile, and group view.">
      {items === null ? (
        <Skeleton height={180} label="Loading activity" />
      ) : items.length === 0 ? (
        <EmptyState
          icon="activity"
          title="No activity yet"
          description="Member posts, likes, and group activity will appear here for moderation."
        />
      ) : (
        <>
          <Table
            caption="Activity items"
            columns={[
              { key: "item", label: "Activity" },
              { key: "visibility", label: "Visibility" },
              { key: "status", label: "Status" },
              { key: "when", label: "When" },
              { key: "action", label: "" },
            ]}
            rows={items.map((item) => ({
              item: (
                <span>
                  <span className="row-title">{item.actorName}</span>
                  <span className="row-sub">{item.content}</span>
                  {item.groupName ? <span className="row-sub">in {item.groupName}</span> : null}
                </span>
              ),
              visibility: item.visibility,
              status: <Badge tone={item.status === "visible" ? "teal" : "amber"}>{item.status}</Badge>,
              when: formatDateTime(item.createdAt, timezone),
              action:
                item.status === "visible" ? (
                  <Button size="small" onClick={() => void hide(item.id)}>
                    Hide
                  </Button>
                ) : (
                  "—"
                ),
            }))}
          />
          <div className="pagination">
            <Button size="small" icon="chevronLeft" disabled={page <= 1} onClick={() => setPage(page - 1)}>
              Previous
            </Button>
            <span>Page {page}</span>
            <Button size="small" icon="chevronRight" disabled={items.length < 20} onClick={() => setPage(page + 1)}>
              Next
            </Button>
          </div>
        </>
      )}
    </Card>
  );
}

function MessagesTab({
  timezone,
  onError,
}: {
  timezone: string;
  onError: (msg: string) => void;
}): React.ReactElement {
  const [threads, setThreads] = useState<SocialThread[] | null>(null);

  const load = useCallback(async () => {
    try {
      setThreads(await api.get<SocialThread[]>("/api/social/messages/threads?page=1&per_page=20"));
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load messages.");
    }
  }, [onError]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <Card title="Messages" sub="Your conversations. Direct messages need friendship or a shared group.">
      {threads === null ? (
        <Skeleton height={180} label="Loading messages" />
      ) : threads.length === 0 ? (
        <EmptyState
          icon="message"
          title="No conversations yet"
          description="Add a friend from the member directory, then start a conversation."
        />
      ) : (
        <Table
          caption="Message threads"
          columns={[
            { key: "with", label: "Conversation with" },
            { key: "last", label: "Last message" },
            { key: "unread", label: "Unread" },
            { key: "when", label: "Updated" },
          ]}
          rows={threads.map((t) => ({
            with: (
              <span>
                <span className="row-title">{t.otherDisplayName}</span>
                <span className="row-sub">@{t.otherLogin}</span>
              </span>
            ),
            last: t.lastMessage ?? "—",
            unread: t.unreadCount > 0 ? <Badge tone="amber">{String(t.unreadCount)}</Badge> : "—",
            when: formatDateTime(t.updatedAt, timezone),
          }))}
        />
      )}
    </Card>
  );
}
