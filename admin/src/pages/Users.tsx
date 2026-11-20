import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { MediaItem, MediaList, Role, User, UserList, UserProfile } from "../lib/types";
import { Alert, Avatar, Badge, Button, Card, EmptyState, Input, Modal, Select, Skeleton, Table } from "../components/elements";

const ROLES: Role[] = ["administrator", "editor", "author", "contributor", "subscriber"];

/** Role descriptions in plain language (§6.3 Users row). */
const ROLE_DESCRIPTIONS: Record<Role, string> = {
  administrator: "Full control — every setting, user, theme and plugin.",
  editor: "Can publish and edit anyone's posts and pages.",
  author: "Can write and publish their own posts.",
  contributor: "Can write posts, but cannot publish them.",
  subscriber: "Can only manage their own profile.",
};

/** Media picker modal (avatar): pick an existing image or upload a new one. */
function AvatarPicker({
  onSelect,
  onClose,
}: {
  onSelect: (item: MediaItem) => void;
  onClose: () => void;
}): React.ReactElement {
  const [list, setList] = useState<MediaList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);

  const load = async (): Promise<void> => {
    try {
      setList(await api.get<MediaList>("/api/media?per_page=24"));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load media.");
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const upload = async (file: File): Promise<void> => {
    setUploading(true);
    setError(null);
    try {
      const item = await api.uploadMedia(file);
      await load();
      onSelect(item);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Upload failed.");
    } finally {
      setUploading(false);
    }
  };

  return (
    <Modal title="Choose avatar" onClose={onClose}>
      {error ? <Alert tone="err" onDismiss={() => setError(null)}>{error}</Alert> : null}
      <Input
        label="Upload a new image"
        type="file"
        accept="image/*"
        disabled={uploading}
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (file) void upload(file);
        }}
      />
      {uploading ? <p className="muted">Uploading…</p> : null}
      {!list ? (
        <Skeleton label="Loading media" />
      ) : list.media.length === 0 ? (
        <EmptyState icon="image" title="No media yet" description="Upload an image above." />
      ) : (
        <div className="media-grid">
          {list.media
            .filter((m) => m.mimeType.startsWith("image/"))
            .map((item) => (
              <button key={item.id} type="button" className="media-card" onClick={() => onSelect(item)} style={{ padding: 0, textAlign: "left" }}>
                <span className="thumb">
                  <img src={item.url} alt={item.alt || item.filename} />
                </span>
                <span className="meta">
                  <span className="name">{item.filename}</span>
                </span>
              </button>
            ))}
        </div>
      )}
      <div className="actions">
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
      </div>
    </Modal>
  );
}

export function UsersPage({ currentUser }: { currentUser: User }): React.ReactElement {
  const [list, setList] = useState<UserList | null>(null);
  const [page, setPage] = useState(1);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [login, setLogin] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("subscriber");
  const [tempPassword, setTempPassword] = useState<{ login: string; password: string } | null>(null);
  // Profile editor (P4 §0.1 A).
  const [editingUser, setEditingUser] = useState<User | null>(null);
  const [profile, setProfile] = useState<UserProfile | null>(null);
  const [profileLoading, setProfileLoading] = useState(false);
  const [displayName, setDisplayName] = useState("");
  const [profileEmail, setProfileEmail] = useState("");
  const [bio, setBio] = useState("");
  const [avatarId, setAvatarId] = useState<number | null>(null);
  const [avatarChanged, setAvatarChanged] = useState(false);
  const [avatarItem, setAvatarItem] = useState<MediaItem | null>(null);
  const [avatarPickerOpen, setAvatarPickerOpen] = useState(false);
  const [profileSaving, setProfileSaving] = useState(false);
  const [profileError, setProfileError] = useState<string | null>(null);
  const [profileNotice, setProfileNotice] = useState<string | null>(null);
  // Own password change (POST /api/auth/change-password).
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [changingPassword, setChangingPassword] = useState(false);
  const [passwordChanged, setPasswordChanged] = useState(false);

  const load = useCallback(async () => {
    try {
      setList(await api.get<UserList>(`/api/users?page=${page}&per_page=20`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load users.");
    }
  }, [page]);

  useEffect(() => {
    void load();
  }, [load]);

  const create = async (e: React.FormEvent): Promise<void> => {
    e.preventDefault();
    setError(null);
    try {
      const created = await api.post<User>("/api/users", {
        login: login.trim(),
        email: email.trim(),
        password,
        role,
      });
      setNotice(`User “${created.login}” created.`);
      setLogin("");
      setEmail("");
      setPassword("");
      setRole("subscriber");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Create failed.");
    }
  };

  const changeRole = async (user: User, next: Role): Promise<void> => {
    setError(null);
    try {
      await api.put<User>(`/api/users/${user.id}/role`, { role: next });
      setNotice(`“${user.login}” is now ${next} — ${ROLE_DESCRIPTIONS[next]}`);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Role change failed.");
    }
  };

  const remove = async (user: User): Promise<void> => {
    if (user.id === currentUser.id) {
      setError("You cannot delete your own account.");
      return;
    }
    if (!window.confirm(`Delete user “${user.login}”?`)) return;
    setError(null);
    try {
      await api.del(`/api/users/${user.id}`);
      setNotice(`Deleted “${user.login}”.`);
      await load();
    } catch (err) {
      // The API refuses with 400 when the user owns posts — show its message.
      setError(err instanceof Error ? err.message : "Delete failed.");
    }
  };

  const resetPassword = async (user: User): Promise<void> => {
    if (!window.confirm(`Reset the password for “${user.login}”? Their sessions will be revoked and they must set a new password on next login.`)) return;
    setError(null);
    try {
      const { temporaryPassword } = await api.post<{ user: User; temporaryPassword: string }>(
        `/api/users/${user.id}/reset-password`,
      );
      setTempPassword({ login: user.login, password: temporaryPassword });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Reset failed.");
    }
  };

  const openProfile = async (user: User): Promise<void> => {
    setEditingUser(user);
    setProfile(null);
    setProfileError(null);
    setProfileNotice(null);
    setAvatarItem(null);
    setAvatarChanged(false);
    setCurrentPassword("");
    setNewPassword("");
    setPasswordChanged(false);
    setProfileLoading(true);
    try {
      const p = await api.getUserProfile(user.id);
      setProfile(p);
      setDisplayName(p.displayName);
      setProfileEmail(p.email);
      setBio(p.bio);
      setAvatarId(p.avatarId);
      if (p.avatarId !== null) {
        try {
          setAvatarItem(await api.getMedia(p.avatarId));
        } catch {
          setAvatarItem(null);
        }
      }
    } catch (err) {
      setProfileError(err instanceof Error ? err.message : "Failed to load the profile.");
    } finally {
      setProfileLoading(false);
    }
  };

  const closeProfile = (): void => {
    setEditingUser(null);
    setProfile(null);
  };

  const saveProfile = async (): Promise<void> => {
    if (!editingUser) return;
    setProfileError(null);
    setProfileNotice(null);
    setProfileSaving(true);
    try {
      const patch: { displayName: string; email: string; bio: string; avatarId?: number | null } = {
        displayName: displayName.trim(),
        email: profileEmail.trim(),
        bio,
      };
      if (avatarChanged) patch.avatarId = avatarId;
      const updated = await api.updateUserProfile(editingUser.id, patch);
      setProfile(updated);
      setAvatarChanged(false);
      setProfileNotice("Profile saved.");
      await load();
    } catch (err) {
      setProfileError(err instanceof Error ? err.message : "Failed to save the profile.");
    } finally {
      setProfileSaving(false);
    }
  };

  const changeOwnPassword = async (): Promise<void> => {
    if (!editingUser) return;
    if (newPassword.length < 8) {
      setProfileError("The new password must be at least 8 characters.");
      return;
    }
    setProfileError(null);
    setChangingPassword(true);
    try {
      await api.changePassword(currentPassword, newPassword);
      setPasswordChanged(true);
    } catch (err) {
      setProfileError(err instanceof Error ? err.message : "Password change failed.");
    } finally {
      setChangingPassword(false);
    }
  };

  const isOwnProfile = editingUser !== null && editingUser.id === currentUser.id;

  return (
    <div>
      {error ? <Alert tone="err" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Card title="Add new user">
        <form onSubmit={(e) => void create(e)}>
          <Input label="Username" value={login} onChange={(e) => setLogin(e.target.value)} required />
          <Input label="Email" type="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
          <Input
            label="Password"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
            minLength={8}
          />
          <Select label="Role" value={role} onChange={(e) => setRole(e.target.value as Role)} hint={ROLE_DESCRIPTIONS[role]}>
            {ROLES.map((r) => (
              <option key={r} value={r}>
                {r} — {ROLE_DESCRIPTIONS[r]}
              </option>
            ))}
          </Select>
          <Button variant="primary" type="submit">
            Add user
          </Button>
        </form>
      </Card>

      <Card title="Users">
        {!list ? (
          <Skeleton label="Loading users" height={160} />
        ) : list.users.length === 0 ? (
          <EmptyState icon="users" title="No users found" />
        ) : (
          <>
            <Table
              caption="Users"
              columns={[
                { key: "user", label: "User" },
                { key: "email", label: "Email" },
                { key: "role", label: "Role" },
                { key: "registered", label: "Registered" },
                { key: "actions", label: "Actions" },
              ]}
              rows={list.users.map((user) => ({
                user: (
                  <span style={{ display: "inline-flex", gap: 8, alignItems: "center" }}>
                    <Avatar name={user.displayName || user.login} size="small" />
                    <span>
                      <strong>{user.displayName || user.login}</strong>{" "}
                      {user.id === currentUser.id ? <Badge tone="blue">You</Badge> : null}
                      <br />
                      <span className="muted">@{user.login}</span>
                    </span>
                  </span>
                ),
                email: user.email,
                role: (
                  <span>
                    <select
                      value={user.role}
                      onChange={(e) => void changeRole(user, e.target.value as Role)}
                      aria-label={`Role for ${user.login}`}
                      style={{ width: "auto" }}
                      title={ROLE_DESCRIPTIONS[user.role]}
                    >
                      {ROLES.map((r) => (
                        <option key={r} value={r}>
                          {r}
                        </option>
                      ))}
                    </select>
                    <br />
                    <span className="muted">{ROLE_DESCRIPTIONS[user.role]}</span>
                  </span>
                ),
                registered: user.registered.slice(0, 10),
                actions: (
                  <span style={{ display: "inline-flex", gap: 6, flexWrap: "wrap" }}>
                    <Button variant="secondary" size="small" onClick={() => void openProfile(user)}>
                      Edit profile
                    </Button>
                    <Button variant="ghost" size="small" onClick={() => void resetPassword(user)}>
                      Reset password
                    </Button>
                    <Button
                      variant="danger"
                      size="small"
                      disabled={user.id === currentUser.id}
                      onClick={() => void remove(user)}
                    >
                      Delete
                    </Button>
                  </span>
                ),
              }))}
            />
            <div style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 12, flexWrap: "wrap" }}>
              <Button variant="secondary" size="small" disabled={page <= 1} onClick={() => setPage(page - 1)}>
                Previous
              </Button>
              <span className="muted">
                Page {list.page} of {list.totalPages} · {list.total} user{list.total === 1 ? "" : "s"}
              </span>
              <Button variant="secondary" size="small" disabled={page >= list.totalPages} onClick={() => setPage(page + 1)}>
                Next
              </Button>
            </div>
          </>
        )}
      </Card>

      {editingUser ? (
        <Modal title={`Edit profile — ${editingUser.login}`} onClose={closeProfile}>
          {profileLoading ? (
            <Skeleton label="Loading profile" height={200} />
          ) : profileError ? (
            <Alert tone="err" onDismiss={() => setProfileError(null)}>{profileError}</Alert>
          ) : profile ? (
            <div>
              {profileNotice ? <Alert tone="ok" onDismiss={() => setProfileNotice(null)}>{profileNotice}</Alert> : null}
              {profile.mustChangePassword ? (
                <Alert tone="warn">
                  This user must set a new password on next login (an admin password reset is pending).
                </Alert>
              ) : null}
              <div style={{ display: "flex", gap: 12, alignItems: "center", marginBottom: 12 }}>
                {avatarItem ? (
                  <img src={avatarItem.url} alt="" style={{ width: 64, height: 64, borderRadius: "50%", objectFit: "cover" }} />
                ) : (
                  <Avatar name={displayName || editingUser.login} size="large" />
                )}
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                  <Button variant="secondary" size="small" onClick={() => setAvatarPickerOpen(true)}>
                    {avatarId === null ? "Choose avatar" : "Change avatar"}
                  </Button>
                  {avatarId !== null ? (
                    <Button
                      variant="ghost"
                      size="small"
                      onClick={() => {
                        setAvatarId(null);
                        setAvatarItem(null);
                        setAvatarChanged(true);
                      }}
                    >
                      Remove avatar
                    </Button>
                  ) : null}
                </div>
              </div>
              <p className="muted" style={{ marginTop: 0 }}>
                The avatar is stored as a media attachment id — pick any image from the media library, or upload a new one.
              </p>
              <Input label="Display name" value={displayName} onChange={(e) => setDisplayName(e.target.value)} required />
              <Input label="Email" type="email" value={profileEmail} onChange={(e) => setProfileEmail(e.target.value)} required />
              <div className="field">
                <label htmlFor="profile-bio">Bio</label>
                <textarea
                  id="profile-bio"
                  value={bio}
                  onChange={(e) => setBio(e.target.value)}
                  rows={3}
                  maxLength={5000}
                  placeholder="A short bio, shown on the author's public profile."
                />
              </div>
              <p className="muted">
                Username <strong>@{editingUser.login}</strong> · Role <strong>{editingUser.role}</strong> —{" "}
                {ROLE_DESCRIPTIONS[editingUser.role]} Change the role from the users table.
              </p>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                <Button variant="primary" disabled={profileSaving} onClick={() => void saveProfile()}>
                  {profileSaving ? "Saving…" : "Save profile"}
                </Button>
                <Button variant="ghost" onClick={closeProfile}>
                  Close
                </Button>
              </div>

              {isOwnProfile ? (
                <div style={{ marginTop: 20, borderTop: "1px solid var(--border)", paddingTop: 16 }}>
                  <h4 style={{ marginTop: 0 }}>Change my password</h4>
                  {passwordChanged ? (
                    <Alert tone="ok">
                      Password changed. Your sessions were revoked — please sign in again.
                      <span style={{ display: "block", marginTop: 8 }}>
                        <Button variant="primary" size="small" onClick={() => window.location.reload()}>
                          Sign in again
                        </Button>
                      </span>
                    </Alert>
                  ) : (
                    <div>
                      <Input
                        label="Current password"
                        type="password"
                        value={currentPassword}
                        onChange={(e) => setCurrentPassword(e.target.value)}
                        autoComplete="current-password"
                      />
                      <Input
                        label="New password (at least 8 characters)"
                        type="password"
                        value={newPassword}
                        onChange={(e) => setNewPassword(e.target.value)}
                        minLength={8}
                        autoComplete="new-password"
                      />
                      <Button
                        variant="secondary"
                        disabled={changingPassword || currentPassword === "" || newPassword === ""}
                        onClick={() => void changeOwnPassword()}
                      >
                        {changingPassword ? "Changing…" : "Change password"}
                      </Button>
                    </div>
                  )}
                </div>
              ) : (
                <p className="muted" style={{ marginTop: 16 }}>
                  To reset this user's password, use “Reset password” in the users table — they get a one-time
                  temporary password and must set a new one on next login.
                </p>
              )}
            </div>
          ) : null}
        </Modal>
      ) : null}

      {avatarPickerOpen && editingUser ? (
        <AvatarPicker
          onSelect={(item) => {
            setAvatarId(item.id);
            setAvatarItem(item);
            setAvatarChanged(true);
            setAvatarPickerOpen(false);
          }}
          onClose={() => setAvatarPickerOpen(false)}
        />
      ) : null}

      {tempPassword ? (
        <Modal title="Temporary password" onClose={() => setTempPassword(null)}>
          <p>
            The temporary password for <strong>{tempPassword.login}</strong> is shown <strong>once</strong>. Copy it
            now and share it securely — it will not be shown again, and it is not stored anywhere in plain text.
            The user must set a new password when they next sign in.
          </p>
          <p style={{ fontSize: 20, fontFamily: "ui-monospace, monospace", wordBreak: "break-all" }}>
            {tempPassword.password}
          </p>
          <div className="actions">
            <Button
              variant="primary"
              onClick={() => {
                void navigator.clipboard?.writeText(tempPassword.password);
              }}
            >
              Copy
            </Button>
            <Button variant="ghost" onClick={() => setTempPassword(null)}>
              Done
            </Button>
          </div>
        </Modal>
      ) : null}
    </div>
  );
}
