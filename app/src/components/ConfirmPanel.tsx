import type { ReactNode } from 'react';

import { Button } from './Button';
import './ConfirmPanel.css';

interface ConfirmPanelProps {
  /** The question, spelling out what will happen. */
  children: ReactNode;
  confirmLabel: string;
  onCancel: () => void;
  onConfirm: () => unknown;
}

/** An inline "are you sure?" box for destructive actions. */
export function ConfirmPanel({ children, confirmLabel, onCancel, onConfirm }: ConfirmPanelProps) {
  return (
    <div className="confirm">
      <div>{children}</div>
      <div className="btn-row">
        <Button size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="danger" size="sm" onClick={onConfirm}>
          {confirmLabel}
        </Button>
      </div>
    </div>
  );
}
