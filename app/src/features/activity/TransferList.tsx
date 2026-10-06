import type { Transfer } from '../../api/types';
import { ListItem, SectionLabel } from '../../components/ListItem';
import { ProgressBar } from '../../components/ProgressBar';
import { useOpenContainingFolder } from '../../hooks/useSyncRoot';
import { formatBytes } from '../../lib/format';
import { baseName } from '../../lib/paths';

/** Uploads and downloads in progress. Rows are keyed so their bars animate smoothly. */
export function TransferList({ transfers }: { transfers: Transfer[] }) {
  const openFolder = useOpenContainingFolder();
  if (!transfers.length) return null;

  return (
    <>
      <SectionLabel>In progress</SectionLabel>
      {transfers.map((t) => {
        const up = t.direction === 'upload';
        const known = t.bytes_total > 0;
        const verb = up ? 'Uploading' : 'Downloading';
        return (
          <ListItem
            key={`${t.direction}:${t.path}`}
            icon={up ? 'upload' : 'download'}
            tone={up ? 'up' : 'down'}
            name={baseName(t.path)}
            title={t.path}
            progress={
              <ProgressBar value={known ? t.bytes_done / t.bytes_total : null} tone={up ? 'accent' : 'download'} />
            }
            sub={known ? `${verb} · ${formatBytes(t.bytes_done)} of ${formatBytes(t.bytes_total)}` : `${verb}…`}
            onClick={() => openFolder(t.path)}
          />
        );
      })}
    </>
  );
}
