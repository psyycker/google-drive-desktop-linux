import * as api from '../../api/commands';
import { Button } from '../../components/Button';
import { Field, InputGroup } from '../../components/Field';
import { errorMessage } from '../../lib/format';
import { useSettings } from '../../state/SettingsContext';
import { useToast } from '../../state/ToastContext';
import { useSettingsFields } from './useSettingsFields';

/** The local Google Drive folder, typed or picked. Used in Settings and onboarding. */
export function FolderField() {
  const { form, edit } = useSettings();
  const { text } = useSettingsFields();
  const toast = useToast();

  const browse = async () => {
    try {
      const picked = await api.pickFolder(form.syncRoot.trim() || undefined);
      if (picked) edit({ syncRoot: picked });
    } catch (e) {
      toast.error(`Couldn’t open the folder picker: ${errorMessage(e)}`);
    }
  };

  return (
    <Field label="Google Drive folder" htmlFor="f-root">
      <InputGroup>
        <input id="f-root" type="text" spellCheck={false} autoComplete="off" {...text('syncRoot')} />
        <Button size="sm" onClick={browse}>
          Browse…
        </Button>
      </InputGroup>
    </Field>
  );
}
