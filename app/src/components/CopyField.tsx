import { useEffect, useRef, useState } from 'react';

import { useToast } from '../state/ToastContext';
import { Button } from './Button';
import './CopyField.css';

/** A read-only text box with a Copy button. */
export function CopyField({ value }: { value: string }) {
  const input = useRef<HTMLInputElement>(null);
  const [copied, setCopied] = useState(false);
  const toast = useToast();

  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(id);
  }, [copied]);

  const copy = () => {
    const fallback = () => {
      input.current?.select();
      try {
        document.execCommand('copy');
        setCopied(true);
      } catch {
        toast.error('Select the link and copy it manually');
      }
    };
    if (navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(value).then(() => setCopied(true), fallback);
    } else {
      fallback();
    }
  };

  return (
    <div className="copy-field">
      <input ref={input} type="text" readOnly value={value} />
      <Button size="sm" onClick={copy}>
        {copied ? 'Copied' : 'Copy'}
      </Button>
    </div>
  );
}
