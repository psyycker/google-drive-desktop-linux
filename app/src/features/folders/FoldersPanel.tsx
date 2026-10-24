import { useCallback, useEffect, useRef, useState } from 'react';

import * as api from '../../api/commands';
import { Button } from '../../components/Button';
import { Card } from '../../components/Card';
import { Icon } from '../../components/Icon';
import { useDaemonCommand } from '../../hooks/useDaemonCommand';
import { cx } from '../../lib/classNames';
import { errorMessage, plural } from '../../lib/format';
import { useDaemon } from '../../state/DaemonContext';
import { useToast } from '../../state/ToastContext';
import { FolderTree } from './FolderTree';
import { changeOf, sameSelection, toggle } from './selection';
import '../settings/SaveBar.css';
import './FoldersPanel.css';

const folders = (n: number) => plural(n, 'folder', 'folders');

/** Which Drive folders sync to this computer: the whole Drive, or the ones ticked. */
export function FoldersPanel() {
  const { status } = useDaemon();
  const command = useDaemonCommand();
  const toast = useToast();
  // `undefined` until loaded; `null` is the whole Drive.
  const [saved, setSaved] = useState<string[] | null | undefined>(undefined);
  const [draft, setDraft] = useState<string[] | null>(null);
  const [confirming, setConfirming] = useState(false);
  // What was ticked before switching to the whole Drive, for switching back.
  const lastChosen = useRef<string[]>([]);

  const dirty = saved !== undefined && !sameSelection(saved, draft);
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;

  const load = useCallback(async () => {
    try {
      const sel = await api.getFolders();
      const paths = sel ? sel.map((f) => f.path).sort() : null;
      setSaved(paths);
      // Changes being made here win over a reload.
      if (!dirtyRef.current) setDraft(paths);
      if (paths) lastChosen.current = paths;
    } catch (e) {
      toast.error(`Couldn’t load the folders to sync: ${errorMessage(e)}`);
    }
  }, [toast]);

  // Also picks up changes made elsewhere, e.g. with `gdrive folders`.
  const selectedCount = status?.selected_folders;
  useEffect(() => {
    void load();
  }, [load, selectedCount]);

  // What a confirmation lists must match the selection it confirms.
  useEffect(() => setConfirming(false), [draft]);

  if (saved === undefined) return null;

  const chooseWhole = () => {
    if (draft) lastChosen.current = draft;
    setDraft(null);
  };

  const apply = async () => {
    await command(api.setFolders(draft), draft === null ? 'Syncing your whole Drive' : 'Folders updated');
    setConfirming(false);
    setSaved(draft);
    void load();
  };

  const change = changeOf(saved, draft);
  const removes = change.narrowsWholeDrive || change.removed.length > 0;
  const summary =
    draft === null
      ? 'Download your whole Drive'
      : change.narrowsWholeDrive
        ? `Keep only ${folders(draft.length)} on this computer`
        : [
            change.added.length > 0 && `${folders(change.added.length)} to download`,
            change.removed.length > 0 && `${folders(change.removed.length)} to remove from this computer`,
          ]
            .filter(Boolean)
            .join(' · ') || 'Unsaved changes';

  return (
    <div className="folders">
      <Card title="Folders to sync">
        <p className="hint folders-intro">
          Choose what to keep on this computer. Everything stays on Google Drive either way.
        </p>
        <div className="modes" role="radiogroup">
          <ModeOption
            active={draft !== null}
            title="Only the folders I choose"
            hint="The rest of your Drive isn’t downloaded."
            onSelect={() => setDraft(lastChosen.current)}
          />
          <ModeOption
            active={draft === null}
            title="My whole Drive"
            hint="Everything in My Drive is downloaded and kept in sync."
            onSelect={chooseWhole}
          />
        </div>
      </Card>

      {draft !== null && (
        <Card title="Choose folders">
          {draft.length === 0 ? (
            <p className="hint chosen-empty">Nothing is chosen yet: tick the folders you want on this computer.</p>
          ) : (
            <div className="chosen">
              {draft.map((f) => (
                <span key={f} className="chip">
                  <span className="chip-name" title={f}>
                    {f}
                  </span>
                  <button
                    type="button"
                    className="chip-remove"
                    aria-label={`Don’t sync ${f}`}
                    onClick={() => setDraft(toggle(draft, f))}
                  >
                    <Icon name="close" />
                  </button>
                </span>
              ))}
            </div>
          )}
          <FolderTree selection={draft} onToggle={(path) => setDraft(toggle(draft, path))} />
          <p className="hint folders-foot">
            A folder comes with everything inside it. Files you add outside the chosen folders stay on this computer
            only.
          </p>
        </Card>
      )}

      {dirty && (
        <div className={cx('savebar', confirming && 'confirming')}>
          {confirming ? (
            <div className="apply-confirm">
              <div>
                {change.narrowsWholeDrive ? (
                  <>
                    Remove everything {draft && draft.length > 0 ? 'except the chosen folders ' : ''}from this
                    computer?
                  </>
                ) : (
                  <>
                    Remove <b>{change.removed.join(', ')}</b> from this computer?
                  </>
                )}
              </div>
              <div className="hint">
                It stays on Google Drive. Files changed here that haven’t been uploaded yet are kept.
              </div>
            </div>
          ) : (
            <span>{summary}</span>
          )}
          <div className="btn-row">
            <Button size="sm" onClick={() => (confirming ? setConfirming(false) : setDraft(saved))}>
              {confirming ? 'Cancel' : 'Discard'}
            </Button>
            {confirming || !removes ? (
              <Button variant={removes ? 'danger' : 'primary'} size="sm" onClick={apply}>
                {removes ? 'Remove and apply' : 'Apply'}
              </Button>
            ) : (
              <Button variant="primary" size="sm" onClick={() => setConfirming(true)}>
                Apply…
              </Button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function ModeOption({
  active,
  title,
  hint,
  onSelect,
}: {
  active: boolean;
  title: string;
  hint: string;
  onSelect: () => void;
}) {
  return (
    <label className={cx('mode', active && 'active')}>
      <input type="radio" name="sync-mode" checked={active} onChange={onSelect} />
      <span>
        <span className="mode-title">{title}</span>
        <span className="hint">{hint}</span>
      </span>
    </label>
  );
}
