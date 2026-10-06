import { useEffect, useState } from 'react';

import * as api from '../../../api/commands';
import { Card } from '../../../components/Card';
import { Checkbox } from '../../../components/Field';
import { useToast } from '../../../state/ToastContext';

export function GeneralCard() {
  const toast = useToast();
  // Applies immediately: it is not part of the config, so it skips the save bar.
  const [autostart, setAutostart] = useState(false);

  useEffect(() => {
    api.getAutostart().then(setAutostart, () => {});
  }, []);

  const toggle = async (on: boolean) => {
    setAutostart(on);
    try {
      await toast.run(
        api.setAutostart(on),
        on ? 'Google Drive will start when you log in' : 'Start on login turned off',
      );
    } catch {
      setAutostart(!on);
    }
  };

  return (
    <Card title="General">
      <Checkbox checked={autostart} onChange={(on) => void toggle(on)}>
        Start Google Drive when I log in
      </Checkbox>
    </Card>
  );
}
