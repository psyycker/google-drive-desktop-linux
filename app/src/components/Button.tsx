import { useRef, useState, type ButtonHTMLAttributes, type MouseEvent } from 'react';

import { cx } from '../lib/classNames';
import './Button.css';

export type ButtonVariant = 'primary' | 'danger' | 'danger-outline' | 'ghost';

export interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'onClick'> {
  variant?: ButtonVariant;
  size?: 'sm';
  /**
   * When this returns a promise, the button stays disabled until it settles. A
   * rejection is swallowed: the action is expected to have reported its own error.
   */
  onClick?: (event: MouseEvent<HTMLButtonElement>) => unknown;
}

export function Button({ variant, size, className, onClick, disabled, type = 'button', ...rest }: ButtonProps) {
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);

  const handleClick = async (event: MouseEvent<HTMLButtonElement>) => {
    if (busyRef.current || !onClick) return;
    const result = onClick(event);
    if (!(result instanceof Promise)) return;
    busyRef.current = true;
    setBusy(true);
    try {
      await result;
    } catch {
      // Already reported by the action.
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  return (
    <button
      type={type}
      className={cx('btn', variant && `btn-${variant}`, size && `btn-${size}`, className)}
      disabled={disabled || busy}
      onClick={handleClick}
      {...rest}
    />
  );
}
