import { useState } from 'react';

import type { Status } from '../../api/types';
import logo from '../../assets/logo.png';
import { Button } from '../../components/Button';
import { useSettings } from '../../state/SettingsContext';
import { useSignIn } from '../../state/SignInContext';
import { useToast } from '../../state/ToastContext';
import { CredentialFields } from '../settings/CredentialFields';
import { FolderField } from '../settings/FolderField';
import { LoginUrlNotice } from '../signin/LoginUrlNotice';
import { Step } from './Step';
import './Onboarding.css';

/** Setup steps shown until an account is signed in. */
export function Onboarding({ status }: { status: Status }) {
  const { form, save } = useSettings();
  const { loginUrl, signIn } = useSignIn();
  const toast = useToast();
  const [editingClient, setEditingClient] = useState(false);

  const needsClient = status.state === 'setup_required';
  const showClientForm = needsClient || editingClient;
  const signingIn = status.state === 'signing_in';
  const signInError = status.state === 'signed_out' ? status.message : null;

  const saveClient = async () => {
    if (!form.clientId.trim() || !form.clientSecret.trim()) {
      toast.error('Enter both the client ID and the client secret');
      return;
    }
    await save('Saved');
    setEditingClient(false);
  };

  return (
    <div className="onboarding">
      <div className="hero">
        <img src={logo} alt="" className="hero-logo" />
        <h1>Welcome to Google Drive for Linux</h1>
        <p>Keep a folder on this computer in sync with your Google Drive.</p>
      </div>

      <Step
        number={1}
        title="Connect your Google Cloud app"
        state={needsClient ? 'active' : 'done'}
        summary={showClientForm ? undefined : 'Client configured'}
        action={
          !showClientForm && (
            <button type="button" className="link-btn" onClick={() => setEditingClient(true)}>
              Edit
            </button>
          )
        }
      >
        <p className="hint">
          Google Drive access uses your own OAuth client, so your files never pass through anyone else’s app.
        </p>
        <CredentialFields />
        <FolderField />
        <div className="btn-row end">
          <Button variant="primary" onClick={saveClient}>
            Save and continue
          </Button>
        </div>
      </Step>

      <Step number={2} title="Sign in to your Google account" state={needsClient ? 'disabled' : 'active'}>
        <p className={signInError ? 'hint error-text' : 'hint'}>
          {signingIn
            ? 'Finish signing in in your browser. This window updates automatically.'
            : signInError || 'Your browser will open so you can allow access to Google Drive.'}
        </p>
        <div className="btn-row">
          <Button variant="primary" onClick={signIn}>
            {signingIn ? 'Restart sign-in' : 'Sign in with Google'}
          </Button>
        </div>
        {loginUrl && <LoginUrlNotice url={loginUrl} />}
      </Step>
    </div>
  );
}
