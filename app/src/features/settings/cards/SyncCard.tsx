import { Card } from '../../../components/Card';
import { Checkbox, Field, FieldPair, InputWithUnit } from '../../../components/Field';
import { FolderField } from '../FolderField';
import { useSettingsFields } from '../useSettingsFields';

export function SyncCard() {
  const { text, flag } = useSettingsFields();

  return (
    <Card title="Sync">
      <FolderField />
      <FieldPair>
        <Field label="Check Drive every" htmlFor="f-poll">
          <InputWithUnit unit="seconds">
            <input id="f-poll" type="number" min={5} max={3600} step={1} {...text('pollSecs')} />
          </InputWithUnit>
        </Field>
        <Field label="Parallel transfers" htmlFor="f-conc">
          <input id="f-conc" type="number" min={1} max={16} step={1} {...text('concurrency')} />
        </Field>
      </FieldPair>
      <FieldPair>
        <Field label="Download limit" htmlFor="f-down-limit">
          <InputWithUnit unit="MB/s">
            <input id="f-down-limit" type="number" min={0} step={0.1} placeholder="Unlimited" {...text('downloadLimit')} />
          </InputWithUnit>
        </Field>
        <Field label="Upload limit" htmlFor="f-up-limit">
          <InputWithUnit unit="MB/s">
            <input id="f-up-limit" type="number" min={0} step={0.1} placeholder="Unlimited" {...text('uploadLimit')} />
          </InputWithUnit>
        </Field>
      </FieldPair>
      <p className="hint">
        Leave a limit empty (or 0) for unlimited. Limits apply to all transfers combined and take effect immediately.
      </p>
      <Checkbox {...flag('localTrash')}>Move files deleted on Drive to the local trash</Checkbox>
      <Field
        htmlFor="f-ignore"
        label={
          <>
            Ignore patterns{' '}
            <span className="hint-inline">
              one per line, e.g. <code>*.tmp</code>
            </span>
          </>
        }
      >
        <textarea id="f-ignore" rows={4} spellCheck={false} {...text('ignore')} />
      </Field>
    </Card>
  );
}
