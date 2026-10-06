import { CopyField } from '../../components/CopyField';
import './LoginUrlNotice.css';

/** Offers the sign-in link in case the browser did not open by itself. */
export function LoginUrlNotice({ url }: { url: string }) {
  return (
    <div className="login-url">
      <div>If your browser didn’t open, copy this link into it:</div>
      <CopyField value={url} />
    </div>
  );
}
