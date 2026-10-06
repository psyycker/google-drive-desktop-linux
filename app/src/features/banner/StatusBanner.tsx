import * as api from '../../api/commands';
import { Button } from '../../components/Button';
import { Icon } from '../../components/Icon';
import { useDaemonCommand } from '../../hooks/useDaemonCommand';
import { isActiveState } from '../../lib/syncStates';
import { useDaemon } from '../../state/DaemonContext';
import { bannerContent } from './bannerContent';
import './StatusBanner.css';

/** The sync state, with Pause/Resume and Sync now. */
export function StatusBanner() {
  const { status, loaded } = useDaemon();
  const command = useDaemonCommand();
  const b = bannerContent(status, loaded);

  const active = !!status && isActiveState(status.state);
  const paused = status?.state === 'paused';

  return (
    <section className="banner" data-tone={b.tone}>
      <div className="banner-icon">
        <Icon name={b.icon} className={b.spin ? 'spin' : undefined} />
      </div>
      <div className="banner-text">
        <div className="banner-title">{b.title}</div>
        <div className="banner-sub" title={typeof b.sub === 'string' ? b.sub : undefined}>
          {b.sub}
        </div>
      </div>
      {active && (
        <div className="banner-actions">
          <Button variant="ghost" size="sm" onClick={() => command(paused ? api.resume() : api.pause())}>
            {paused ? 'Resume' : 'Pause'}
          </Button>
          {!paused && (
            <Button variant="ghost" size="sm" onClick={() => command(api.syncNow(), 'Checking for changes…')}>
              Sync now
            </Button>
          )}
        </div>
      )}
    </section>
  );
}
