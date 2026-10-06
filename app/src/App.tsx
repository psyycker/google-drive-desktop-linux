import { useEffect, useState } from 'react';

import { Badge, Tabs } from './components/Tabs';
import { ActivityPanel } from './features/activity/ActivityPanel';
import { StatusBanner } from './features/banner/StatusBanner';
import { ErrorsPanel } from './features/errors/ErrorsPanel';
import { AppHeader } from './features/header/AppHeader';
import { Onboarding } from './features/onboarding/Onboarding';
import { SettingsPanel } from './features/settings/SettingsPanel';
import { isOnboardingState } from './lib/syncStates';
import { useDaemon } from './state/DaemonContext';
import { useSettings } from './state/SettingsContext';
import './App.css';

type TabId = 'activity' | 'errors' | 'settings';

export function App() {
  const { status } = useDaemon();
  const { reloadIfClean } = useSettings();
  const onboarding = status && isOnboardingState(status.state) ? status : null;

  // Moving between onboarding and the main view (signing in or out) can change the
  // config behind the form's back.
  const inOnboarding = !!onboarding;
  useEffect(() => {
    reloadIfClean();
  }, [inOnboarding, reloadIfClean]);

  return (
    <div className="app">
      <AppHeader />
      {onboarding ? <Onboarding status={onboarding} /> : <MainView />}
    </div>
  );
}

function MainView() {
  const { status } = useDaemon();
  const [tab, setTab] = useState<TabId>('activity');

  return (
    <>
      <StatusBanner />
      <div className="main">
        <Tabs
          active={tab}
          onSelect={setTab}
          tabs={[
            { id: 'activity', label: 'Activity' },
            {
              id: 'errors',
              label: (
                <>
                  Errors <Badge count={status?.errors.length ?? 0} />
                </>
              ),
            },
            { id: 'settings', label: 'Settings' },
          ]}
        />
        <section className="panel" role="tabpanel" key={tab}>
          {tab === 'activity' && <ActivityPanel />}
          {tab === 'errors' && <ErrorsPanel />}
          {tab === 'settings' && <SettingsPanel />}
        </section>
      </div>
    </>
  );
}
