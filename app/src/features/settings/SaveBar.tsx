import { Button } from '../../components/Button';
import { useSettings } from '../../state/SettingsContext';
import './SaveBar.css';

/** Floats at the bottom of Settings while there are unsaved edits. */
export function SaveBar() {
  const { discard, save } = useSettings();
  return (
    <div className="savebar">
      <span>Unsaved changes</span>
      <div className="btn-row">
        <Button size="sm" onClick={discard}>
          Discard
        </Button>
        <Button variant="primary" size="sm" onClick={() => save()}>
          Save
        </Button>
      </div>
    </div>
  );
}
