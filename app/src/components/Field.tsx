import type { ReactNode } from 'react';

import './Field.css';

/** A labelled form control. */
export function Field({ label, htmlFor, children }: { label: ReactNode; htmlFor: string; children: ReactNode }) {
  return (
    <div className="field">
      <label htmlFor={htmlFor}>{label}</label>
      {children}
    </div>
  );
}

/** Two fields side by side. */
export const FieldPair = ({ children }: { children: ReactNode }) => <div className="field-pair">{children}</div>;

/** An input with a button (or another control) next to it. */
export const InputGroup = ({ children }: { children: ReactNode }) => <div className="input-group">{children}</div>;

/** An input followed by a unit, e.g. "seconds". */
export function InputWithUnit({ unit, children }: { unit: string; children: ReactNode }) {
  return (
    <div className="input-suffix">
      {children}
      <span>{unit}</span>
    </div>
  );
}

interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  children: ReactNode;
}

export function Checkbox({ checked, onChange, children }: CheckboxProps) {
  return (
    <label className="check">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>{children}</span>
    </label>
  );
}
