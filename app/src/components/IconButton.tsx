import { Icon, type IconName } from './Icon';
import './Button.css';

interface IconButtonProps {
  icon: IconName;
  /** Tooltip and accessible name. */
  label: string;
  disabled?: boolean;
  onClick: () => void;
}

export function IconButton({ icon, label, disabled, onClick }: IconButtonProps) {
  return (
    <button type="button" className="icon-btn" title={label} aria-label={label} disabled={disabled} onClick={onClick}>
      <Icon name={icon} />
    </button>
  );
}
