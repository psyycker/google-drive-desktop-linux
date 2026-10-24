import { useCallback, useEffect, useRef, useState } from 'react';

import * as api from '../../api/commands';
import type { FolderEntry } from '../../api/types';
import { Icon } from '../../components/Icon';
import { cx } from '../../lib/classNames';
import { errorMessage } from '../../lib/format';
import { checkOf } from './selection';

type Listing = { folders: FolderEntry[] } | { error: string } | null;

/** The folders inside a Drive folder, loaded when `load` is called. */
function useListing(path: string) {
  const [listing, setListing] = useState<Listing>(null);
  const load = useCallback(async () => {
    try {
      setListing({ folders: await api.browseFolders(path) });
    } catch (e) {
      setListing({ error: errorMessage(e) });
    }
  }, [path]);
  return [listing, load] as const;
}

/** A checkbox that can also show "partly". */
function TriCheckbox({
  checked,
  partly,
  disabled,
  title,
  onChange,
}: {
  checked: boolean;
  partly: boolean;
  disabled?: boolean;
  title?: string;
  onChange: () => void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = partly;
  }, [partly]);
  return <input ref={ref} type="checkbox" checked={checked} disabled={disabled} title={title} onChange={onChange} />;
}

interface TreeProps {
  selection: string[];
  onToggle: (path: string) => void;
}

function FolderNode({ entry, depth, selection, onToggle }: TreeProps & { entry: FolderEntry; depth: number }) {
  const [open, setOpen] = useState(false);
  const [listing, load] = useListing(entry.path);
  const { checked, partly, includedWith } = checkOf(selection, entry.path);

  const expand = () => {
    if (!open && !listing) void load();
    setOpen(!open);
  };

  return (
    <li>
      <div className="tree-row" style={{ paddingLeft: 8 + depth * 20 }}>
        <button
          type="button"
          className={cx('tree-toggle', open && 'open')}
          aria-label={open ? `Collapse ${entry.name}` : `Expand ${entry.name}`}
          aria-expanded={open}
          onClick={expand}
        >
          <Icon name="chevronRight" />
        </button>
        <label className="tree-label">
          <TriCheckbox
            checked={checked}
            partly={partly}
            disabled={includedWith !== null}
            title={includedWith ? `Included with ${includedWith}` : undefined}
            onChange={() => onToggle(entry.path)}
          />
          <Icon name="folder" className="tree-folder" />
          <span className="tree-name">{entry.name}</span>
        </label>
      </div>
      {open && <Children listing={listing} depth={depth + 1} selection={selection} onToggle={onToggle} retry={load} />}
    </li>
  );
}

function Children({
  listing,
  depth,
  selection,
  onToggle,
  retry,
}: TreeProps & { listing: Listing; depth: number; retry: () => void }) {
  const pad = { paddingLeft: 8 + depth * 20 + 28 };
  if (!listing) return <div className="tree-note" style={pad}>Loading…</div>;
  if ('error' in listing) {
    return (
      <div className="tree-note error-text" style={pad}>
        {listing.error}{' '}
        <button type="button" className="link-btn" onClick={retry}>
          Retry
        </button>
      </div>
    );
  }
  if (listing.folders.length === 0) return <div className="tree-note" style={pad}>No folders inside</div>;
  return (
    <ul className="tree">
      {listing.folders.map((f) => (
        <FolderNode key={f.path} entry={f} depth={depth} selection={selection} onToggle={onToggle} />
      ))}
    </ul>
  );
}

/**
 * The folders of the whole Drive, loaded level by level as they are expanded. While
 * the daemon is still reading the Drive (just after signing in), the top level is
 * retried until it can be listed.
 */
export function FolderTree({ selection, onToggle }: TreeProps) {
  const [listing, load] = useListing('');
  const failed = !!listing && 'error' in listing;

  useEffect(() => {
    void load();
  }, [load]);
  useEffect(() => {
    if (!failed) return;
    const timer = window.setInterval(() => void load(), 3000);
    return () => window.clearInterval(timer);
  }, [failed, load]);

  // The daemon's wording for a Drive it hasn't listed yet (see selection.rs).
  if (listing && 'error' in listing && listing.error.includes("hasn't been read yet")) {
    return (
      <div className="tree-wait">
        <Icon name="loader" className="spin" />
        <span>Reading your Drive… the list of folders appears as soon as it’s done.</span>
      </div>
    );
  }
  return (
    <div className="tree-box">
      <Children listing={listing} depth={0} selection={selection} onToggle={onToggle} retry={load} />
    </div>
  );
}
