import { useState } from "react";
import type { RepoEdit, RepoList } from "../proto";
import { repoEdit, repoList, toast } from "../store";
import { useCached } from "../cache";
import { addedRepo, repoChoices } from "../lib/repos";

/** The daemon's one list of repositories (backlog #240), and a way to change it. */
export function useRepos(): { list: RepoList | null; edit: (e: RepoEdit) => Promise<RepoList | null> } {
  const { data: list, reload } = useCached("repos", repoList);
  const edit = async (e: RepoEdit) => {
    try {
      const after = await repoEdit(e);
      await reload();
      return after;
    } catch (err) {
      toast((err as Error).message);
      return null;
    }
  };
  return { list, edit };
}

const ADD = "\u0000add";

/** A repository select whose last option adds one by path, in place. */
export function RepoSelect({
  value,
  onChange,
  none,
  disabled,
  label = "Repository",
}: {
  value: string;
  onChange: (repo: string) => void;
  /** The option for no repository, when there is one. */
  none?: string;
  disabled?: boolean;
  label?: string;
}) {
  const { list, edit } = useRepos();
  const [adding, setAdding] = useState(false);
  const [path, setPath] = useState("");
  const choices = repoChoices(list, value || null);

  const add = async () => {
    const p = path.trim();
    if (!p) return;
    const after = await edit({ do: "add", path: p });
    if (after) {
      const added = addedRepo(list, after, p);
      if (added) onChange(added);
      setAdding(false);
      setPath("");
    }
  };

  return (
    <>
      <select
        aria-label={label}
        value={adding ? ADD : value}
        disabled={disabled}
        onChange={(e) => {
          if (e.target.value === ADD) {
            setAdding(true);
          } else {
            setAdding(false);
            onChange(e.target.value);
          }
        }}
      >
        {none !== undefined && <option value="">{none}</option>}
        {choices.map((c) => (
          <option key={c.repo} value={c.repo} title={c.repo}>
            {c.label}
          </option>
        ))}
        <option value={ADD}>Add a repository…</option>
      </select>
      {adding && (
        <div className="field-row repo-add">
          <input
            autoFocus
            value={path}
            onChange={(e) => setPath(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void add();
              }
              if (e.key === "Escape") setAdding(false);
            }}
            placeholder="~/path/to/repo"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
          />
          <button className="ghost" disabled={!path.trim()} onClick={() => void add()}>
            Add
          </button>
        </div>
      )}
    </>
  );
}
