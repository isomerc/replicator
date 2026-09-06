import { useState } from "react";
import { api } from "../api";
import type { CharacterEntry, Group } from "../types";
import { useI18n } from "../i18n";

// No eveRunning prop on purpose: group CRUD only touches the app's own
// database, so it stays allowed while a client is up.
interface Props {
  groups: Group[];
  charById: Map<number, CharacterEntry>;
  characters: CharacterEntry[];
  onAfterMutation: (msg: string) => void;
  onError: (e: string) => void;
}

export function GroupsView({
  groups,
  charById,
  characters,
  onAfterMutation,
  onError,
}: Props) {
  const { t } = useI18n();
  const [editing, setEditing] = useState<Group | null>(null);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);

  async function createGroup() {
    if (creating || !newName.trim()) return;
    setCreating(true);
    try {
      await api.createGroup(newName.trim());
      setNewName("");
      onAfterMutation(t("msg_created_group", { name: newName.trim() }));
    } catch (e) {
      onError(String(e));
    } finally {
      setCreating(false);
    }
  }

  async function removeGroup(g: Group) {
    if (!confirm(t("confirm_delete_group", { name: g.name }))) return;
    try {
      await api.deleteGroup(g.id);
      onAfterMutation(t("msg_deleted_group", { name: g.name }));
    } catch (e) {
      onError(String(e));
    }
  }

  return (
    <>
      <div className="action-bar">
        <input
          type="text"
          placeholder={t("new_group_placeholder")}
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          style={{ maxWidth: 280 }}
          onKeyDown={(e) => {
            if (e.key === "Enter") createGroup();
          }}
        />
        <button
          className="primary"
          onClick={createGroup}
          disabled={creating || !newName.trim()}
        >
          {t("create")}
        </button>
        <div className="spacer" style={{ flex: 1 }} />
        <span className="muted">
          {groups.length === 1
            ? t("one_group")
            : t("n_groups", { n: groups.length })}
        </span>
      </div>

      {groups.length === 0 ? (
        <div className="card">
          <h3>{t("groups_empty_title")}</h3>
          <p className="help">{t("groups_empty_help")}</p>
        </div>
      ) : (
        <div className="list">
          {groups.map((g) => (
            <div key={g.id} className="list-item">
              <strong>{g.name}</strong>
              <span className="meta">
                {g.member_ids.length === 1
                  ? t("one_member")
                  : t("n_members", { n: g.member_ids.length })}
                {g.member_ids.length > 0 &&
                  " · " +
                    g.member_ids
                      .slice(0, 3)
                      .map((id) => charById.get(id)?.name ?? `#${id}`)
                      .join(", ") +
                    (g.member_ids.length > 3 ? "…" : "")}
              </span>
              <div className="grow" />
              <button onClick={() => setEditing(g)}>{t("edit_members")}</button>
              <button className="danger" onClick={() => removeGroup(g)}>
                {t("delete")}
              </button>
            </div>
          ))}
        </div>
      )}

      {editing && (
        <GroupMembersDialog
          group={editing}
          characters={characters}
          onClose={() => setEditing(null)}
          onConfirm={async (ids) => {
            try {
              await api.setGroupMembers(editing.id, ids);
              onAfterMutation(t("msg_updated_group", { name: editing.name }));
              setEditing(null);
            } catch (e) {
              onError(String(e));
              setEditing(null);
            }
          }}
        />
      )}
    </>
  );
}

interface MembersProps {
  group: Group;
  characters: CharacterEntry[];
  onClose: () => void;
  onConfirm: (ids: number[]) => void;
}

function GroupMembersDialog({ group, characters, onClose, onConfirm }: MembersProps) {
  const { t } = useI18n();
  const [members, setMembers] = useState<Set<number>>(new Set(group.member_ids));
  function toggle(id: number) {
    setMembers((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{t("members_of", { name: group.name })}</h2>
        <div className="list" style={{ maxHeight: "55vh", overflow: "auto" }}>
          {characters.map((c) => (
            <label key={c.id} className="list-item" style={{ cursor: "pointer" }}>
              <input
                type="checkbox"
                className="checkbox"
                checked={members.has(c.id)}
                onChange={() => toggle(c.id)}
              />
              <span className="grow">
                {c.name ?? t("character_n", { id: c.id })}
                <span className="meta"> · #{c.id}</span>
              </span>
            </label>
          ))}
        </div>
        <div className="actions">
          <button onClick={onClose}>{t("cancel")}</button>
          <button className="primary" onClick={() => onConfirm(Array.from(members))}>
            {t("save_n", { n: members.size })}
          </button>
        </div>
      </div>
    </div>
  );
}
