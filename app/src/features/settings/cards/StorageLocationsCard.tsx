import { useEffect, useRef, useState } from 'react';

import * as api from '../../../api/commands';
import { Button } from '../../../components/Button';
import { Card, SettingRow } from '../../../components/Card';
import { ConfirmPanel } from '../../../components/ConfirmPanel';
import { Field, InputGroup } from '../../../components/Field';
import { ProgressBar } from '../../../components/ProgressBar';
import { useDaemonCommand } from '../../../hooks/useDaemonCommand';
import { errorMessage, formatBytes } from '../../../lib/format';
import { useDaemon } from '../../../state/DaemonContext';
import { useSettings } from '../../../state/SettingsContext';
import { useToast } from '../../../state/ToastContext';
import './StorageLocationsCard.css';

/** The Drive-relative path of `abs`, or `null` when it isn't inside the Drive folder. */
function driveRelative(abs: string, root: string): string | null {
  const base = root.replace(/\/+$/, '');
  if (!abs.startsWith(`${base}/`)) return null;
  return abs.slice(base.length + 1).replace(/\/+$/, '') || null;
}

/** Drives besides the main one, and which Drive folders live on them. */
export function StorageLocationsCard() {
  const { status } = useDaemon();
  const { config, form, edit, reloadIfClean } = useSettings();
  const command = useDaemonCommand();
  const toast = useToast();

  const [newName, setNewName] = useState('');
  const [newRoot, setNewRoot] = useState('');
  const [moving, setMoving] = useState<{ folder: string; to: string } | null>(null);
  const [confirmBack, setConfirmBack] = useState<string | null>(null);

  // Moves change the placements on the daemon side: reload once one finishes.
  const relocation = status?.relocation ?? null;
  const wasMoving = useRef(false);
  useEffect(() => {
    if (wasMoving.current && !relocation) reloadIfClean();
    wasMoving.current = !!relocation;
  }, [relocation, reloadIfClean]);

  const placements = config?.placements ?? [];
  const saved = config?.locations ?? [];
  const syncRoot = config?.sync_root ?? '';
  const busy = !!relocation;

  const browse = async (defaultPath: string, title: string) => {
    try {
      return await api.pickFolder(defaultPath || undefined, title);
    } catch (e) {
      toast.error(`Couldn’t open the folder picker: ${errorMessage(e)}`);
      return null;
    }
  };

  const addDrive = () => {
    const name = newName.trim();
    const root = newRoot.trim();
    if (!name || !root) {
      toast.error('Give the drive a name and a folder');
      return;
    }
    edit({ locations: [...form.locations, { name, root }] });
    setNewName('');
    setNewRoot('');
  };

  const chooseFolder = async () => {
    const picked = await browse(syncRoot, 'Choose a folder inside your Google Drive folder');
    if (!picked) return;
    const folder = driveRelative(picked, syncRoot);
    if (!folder) {
      toast.error('Pick a folder inside your Google Drive folder');
      return;
    }
    setMoving({ folder, to: saved[0]?.name ?? '' });
  };

  const startMove = async (folder: string, location: string | null) => {
    await command(api.relocateFolder(folder, location));
    setMoving(null);
    setConfirmBack(null);
  };

  return (
    <Card title="Other drives">
      <p className="hint locations-intro">
        Keep some Drive folders on another disk, such as a faster SSD. They still show up in your Google Drive folder
        through a link, and sync as usual.
      </p>

      {relocation && (
        <div className="relocation">
          <div>
            Moving <b>{relocation.folder}</b> to {relocation.to ?? 'the main drive'}
            {relocation.bytes_total > 0 &&
              ` · ${formatBytes(relocation.bytes_done)} of ${formatBytes(relocation.bytes_total)}`}
          </div>
          <ProgressBar value={relocation.bytes_total > 0 ? relocation.bytes_done / relocation.bytes_total : null} />
          <div className="hint">Syncing is paused until the move is done.</div>
        </div>
      )}

      {form.locations.map((loc) => {
        const used = placements.some((p) => p.location === loc.name);
        return (
          <div key={loc.name} className="location-row">
            <SettingRow title={loc.name} hint={<span className="selectable">{loc.root}</span>}>
              <Button
                size="sm"
                variant="ghost"
                disabled={used}
                title={used ? 'Move its folders back to the main drive first' : undefined}
                onClick={() =>
                  edit({
                    locations: form.locations.filter((l) => l.name !== loc.name),
                  })
                }
              >
                Remove
              </Button>
            </SettingRow>
          </div>
        );
      })}

      <div className="add-drive">
        <Field label="Add a drive" htmlFor="f-loc-name">
          <InputGroup>
            <input
              id="f-loc-name"
              className="loc-name"
              type="text"
              placeholder="Name, e.g. SSD"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
            />
            <input
              type="text"
              placeholder="/mnt/ssd/GoogleDrive"
              spellCheck={false}
              value={newRoot}
              onChange={(e) => setNewRoot(e.target.value)}
            />
            <Button
              size="sm"
              onClick={async () => {
                const picked = await browse(newRoot, 'Choose a folder on the other drive');
                if (picked) setNewRoot(picked);
              }}
            >
              Browse…
            </Button>
            <Button size="sm" onClick={addDrive}>
              Add
            </Button>
          </InputGroup>
        </Field>
        <p className="hint">New drives are kept once you save settings.</p>
      </div>

      {(placements.length > 0 || saved.length > 0) && <div className="locations-label">Folders on other drives</div>}
      {placements.map((p) => (
        <SettingRow key={p.folder} title={p.folder} hint={`On ${p.location}`}>
          <Button size="sm" disabled={busy} onClick={() => setConfirmBack(p.folder)}>
            Move back…
          </Button>
        </SettingRow>
      ))}
      {confirmBack && (
        <ConfirmPanel
          confirmLabel="Move back"
          onCancel={() => setConfirmBack(null)}
          onConfirm={() => startMove(confirmBack, null)}
        >
          Move <b>{confirmBack}</b> back into your Google Drive folder? Its files are copied over, then removed from the
          other drive. Syncing pauses meanwhile.
        </ConfirmPanel>
      )}

      {saved.length > 0 && !moving && (
        <Button size="sm" className="move-start" disabled={busy} onClick={chooseFolder}>
          Move a folder to another drive…
        </Button>
      )}
      {moving && (
        <ConfirmPanel
          confirmLabel="Move"
          onCancel={() => setMoving(null)}
          onConfirm={() => (moving.to ? startMove(moving.folder, moving.to) : toast.error('Choose a drive'))}
        >
          <div className="move-form">
            Move <b>{moving.folder}</b> to{' '}
            <select value={moving.to} onChange={(e) => setMoving({ ...moving, to: e.target.value })}>
              {saved.map((l) => (
                <option key={l.name} value={l.name}>
                  {l.name}
                </option>
              ))}
            </select>
            ?
          </div>
          <div className="hint">
            Its files are copied there, then removed from here; a link takes their place. Drive itself is not changed.
            Syncing pauses meanwhile.
          </div>
        </ConfirmPanel>
      )}
    </Card>
  );
}
