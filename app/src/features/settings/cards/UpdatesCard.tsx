import * as api from '../../../api/commands';
import { Button } from '../../../components/Button';
import { Card, SettingRow } from '../../../components/Card';
import { Checkbox } from '../../../components/Field';
import { ProgressBar } from '../../../components/ProgressBar';
import { useDaemonCommand } from '../../../hooks/useDaemonCommand';
import { useDaemon } from '../../../state/DaemonContext';
import { useToast } from '../../../state/ToastContext';
import { useSettingsFields } from '../useSettingsFields';
import { updateView, type UpdateAction } from './updateView';
import './UpdatesCard.css';

export function UpdatesCard() {
  const { status, appVersion } = useDaemon();
  const command = useDaemonCommand();
  const toast = useToast();
  const { flag } = useSettingsFields();
  const view = updateView(status, appVersion);

  const runAction = (action: UpdateAction) =>
    toast.fire(action.kind === 'restart' ? api.restartApp() : api.openUrl(action.url));

  return (
    <Card title="Updates" className="updates-card">
      <SettingRow title={<span className="update-line">{view.line}</span>} hint={view.sub}>
        <Button size="sm" onClick={() => command(api.checkForUpdates(), 'Checking for updates…')}>
          Check now
        </Button>
        {view.action && (
          <Button variant="primary" size="sm" onClick={() => view.action && runAction(view.action)}>
            {view.action.label}
          </Button>
        )}
      </SettingRow>
      {view.progress !== null && <ProgressBar value={view.progress} className="update-progress" />}
      <Checkbox {...flag('autoUpdate')}>Install updates automatically when syncing is idle</Checkbox>
    </Card>
  );
}
