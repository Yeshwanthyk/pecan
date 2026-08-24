/** Compact settings surface built only from behavior Pecan actually owns. */
import {
  FolderGit2Icon,
  PanelsTopLeftIcon,
  MoonIcon,
  RadioIcon,
  SparklesIcon,
  SunIcon,
} from "lucide-react";
import { useState, type ReactNode } from "react";

import {
  api,
  readAiTitleGenerationEnabled,
  readTitleModelPreset,
  saveAiTitleGenerationEnabled,
  saveTitleModelPreset,
  type TitleModelPreset,
} from "~/api/client";
import { Button } from "~/components/ui/button";
import { Switch } from "~/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "~/components/ui/select";
import { cn } from "~/lib/utils";
import { useApp, type ThemeName } from "~/store";
import {
  CURATED_CODE_FONTS,
  CURATED_UI_FONTS,
  SYSTEM_CODE,
  SYSTEM_UI,
  loadInstalledFonts,
  readCodeFont,
  readUiFont,
  saveCodeFont,
  saveUiFont,
} from "~/fonts";

const EMPTY_PROJECTS: Array<{ cwd: string; name: string }> = [];
const DEFAULT_TITLE_MODEL = {
  value: "luna-low",
  label: "Luna · Low",
  detail: "Fast, concise titles",
  recommended: true,
} as const;
const TITLE_MODELS: Array<{
  value: TitleModelPreset;
  label: string;
  detail: string;
  recommended?: boolean;
}> = [
  DEFAULT_TITLE_MODEL,
  {
    value: "luna-medium",
    label: "Luna · Medium",
    detail: "More reasoning for long threads",
  },
  {
    value: "sol-low",
    label: "Sol · Low",
    detail: "Nuanced wording from a larger model",
  },
];

export function SettingsPage() {
  const theme = useApp((state) => state.theme);
  const connected = useApp((state) => state.connected);
  const sessions = useApp((state) => state.sessions);
  const projects = useApp((state) => state.bootstrap?.projects ?? EMPTY_PROJECTS);
  const uiPlugins = useApp((state) => state.bootstrap?.uiPlugins ?? []);
  const [removing, setRemoving] = useState<string | null>(null);
  const [changingPlugin, setChangingPlugin] = useState<string | null>(null);
  const [aiTitlesEnabled, setAiTitlesEnabled] = useState(
    readAiTitleGenerationEnabled,
  );
  const [titleModel, setTitleModel] = useState<TitleModelPreset>(readTitleModelPreset);
  const [uiFont, setUiFont] = useState(readUiFont);
  const [codeFont, setCodeFont] = useState(readCodeFont);

  async function removeProject(cwd: string) {
    setRemoving(cwd);
    try {
      await api.removeProject(cwd);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
    } catch (error) {
      reportError(error instanceof Error ? error.message : String(error));
    } finally {
      setRemoving(null);
    }
  }

  async function setPluginEnabled(id: string, enabled: boolean) {
    setChangingPlugin(id);
    try {
      await api.setUiPlugin(id, enabled);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
    } catch (error) {
      reportError(error instanceof Error ? error.message : String(error));
    } finally {
      setChangingPlugin(null);
    }
  }

  return (
    <div className="min-h-0 flex-1 overflow-y-auto" aria-labelledby="settings-title">
      <div className="mx-auto w-full max-w-[46rem] px-5 py-8 md:px-8 md:py-12">
        <div className="mb-9">
          <h1 className="text-xl font-semibold tracking-tight" id="settings-title">
            Settings
          </h1>
          <p className="mt-1 max-w-[62ch] text-sm leading-6 text-muted-foreground">
            Local preferences and runtime details for this Pecan installation.
          </p>
        </div>

        <SettingsSection icon={<SunIcon />} title="Appearance">
          <SettingsRow
            control={<ThemeControl theme={theme} />}
            description="Stored in this browser and applied immediately."
            title="Theme"
          />
          <FontControl code={codeFont} kind="code" onChange={setCodeFont} title="Code font" />
          <FontControl code={uiFont} kind="ui" onChange={setUiFont} title="UI font" />
        </SettingsSection>

        <SettingsSection icon={<SparklesIcon />} title="Thread titles">
          <SettingsRow
            control={
              <Switch
                aria-label="Allow AI title generation"
                checked={aiTitlesEnabled}
                onCheckedChange={(enabled) => {
                  setAiTitlesEnabled(enabled);
                  saveAiTitleGenerationEnabled(enabled);
                }}
              />
            }
            description="Off by default. Pecan uses the first request as the title and makes no model call."
            title="AI title generation"
          />
          {aiTitlesEnabled ? (
            <SettingsRow
            control={
              <TitleModelControl
                onChange={(preset) => {
                  setTitleModel(preset);
                  saveTitleModelPreset(preset);
                }}
                value={titleModel}
              />
            }
            description="Manual only. Clicking a title refresh sends bounded thread context to this model and uses tokens."
            title="Generation model"
          />
          ) : null}
        </SettingsSection>

        <SettingsSection icon={<RadioIcon />} title="Runtime">
          <SettingsRow
            control={
              <span className="flex items-center gap-2 text-xs font-medium">
                <span
                  aria-hidden
                  className={cn(
                    "size-2 rounded-full",
                    connected ? "bg-success" : "bg-warning",
                  )}
                />
                {connected ? "Connected" : "Reconnecting"}
              </span>
            }
            description="Live updates between the local Pecan server and this browser."
            title="Event stream"
          />
          <SettingsRow
            control={<span className="text-xs tabular-nums">{sessions.length}</span>}
            description="Pi sessions currently available in the local index."
            title="Loaded sessions"
          />
        </SettingsSection>

        <SettingsSection icon={<PanelsTopLeftIcon />} title="Session UI">
          {uiPlugins.map((plugin) => (
            <SettingsRow
              control={
                <Switch
                  aria-label={`Show ${plugin.name} in sessions`}
                  checked={plugin.enabled}
                  disabled={
                    changingPlugin === plugin.id ||
                    !plugin.detected ||
                    !plugin.sourceEnabled
                  }
                  onCheckedChange={(checked) =>
                    void setPluginEnabled(plugin.id, checked)
                  }
                />
              }
              description={pluginDescription(plugin)}
              key={plugin.id}
              title={plugin.name}
            />
          ))}
        </SettingsSection>

        <SettingsSection
          icon={<FolderGit2Icon />}
          title="Linked projects"
        >
          {projects.length > 0 ? (
            projects.map((project) => (
              <SettingsRow
                control={
                  <Button
                    disabled={removing === project.cwd}
                    className="min-h-11 md:min-h-8"
                    onClick={() => void removeProject(project.cwd)}
                    size="xs"
                    variant="ghost"
                  >
                    {removing === project.cwd ? "Removing…" : "Remove"}
                  </Button>
                }
                description={project.cwd}
                key={project.cwd}
                title={project.name}
              />
            ))
          ) : (
            <p className="py-4 text-sm text-muted-foreground">
              Link a project from the sidebar to include its sessions.
            </p>
          )}
        </SettingsSection>
      </div>
    </div>
  );
}

function pluginDescription(plugin: {
  detected: boolean;
  sourceEnabled: boolean;
  enabled: boolean;
  description: string;
}) {
  if (!plugin.detected) return "Install Pi Tasks to make this view available.";
  if (!plugin.sourceEnabled) return "Installed, but disabled in your Pi settings.";
  if (plugin.enabled) return "Read-only typed task data is shown inside each thread.";
  return `${plugin.description} Off until you choose to enable it.`;
}

function TitleModelControl({
  value,
  onChange,
}: {
  value: TitleModelPreset;
  onChange: (value: TitleModelPreset) => void;
}) {
  const selected = TITLE_MODELS.find((model) => model.value === value) ?? DEFAULT_TITLE_MODEL;

  return (
    <Select
      onValueChange={(next) => {
        if (next && TITLE_MODELS.some((model) => model.value === next)) {
          onChange(next as TitleModelPreset);
        }
      }}
      value={value}
    >
      <SelectTrigger
        aria-label="Title generation model"
        className="min-h-11 w-[min(17rem,calc(100vw-2.5rem))] sm:min-h-9 sm:w-64"
      >
        <SelectValue>
          <span className="flex min-w-0 items-center gap-2">
            <span className="truncate font-medium">{selected.label}</span>
            {selected.recommended ? (
              <span className="rounded-sm bg-foreground/[0.07] px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">
                Recommended
              </span>
            ) : null}
          </span>
        </SelectValue>
      </SelectTrigger>
      <SelectContent align="end" className="w-[min(19rem,calc(100vw-2rem))]" matchTriggerWidth={false}>
        {TITLE_MODELS.map((model) => (
          <SelectItem className="min-h-12 py-2" key={model.value} value={model.value}>
            <span className="flex min-w-0 flex-col gap-0.5">
              <span className="flex items-center gap-2 font-medium">
                {model.label}
                {model.recommended ? (
                  <span className="text-[10px] font-normal text-muted-foreground">
                    Recommended
                  </span>
                ) : null}
              </span>
              <span className="text-xs font-normal text-muted-foreground">
                {model.detail}
              </span>
            </span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function SettingsSection({
  title,
  icon,
  children,
}: {
  title: string;
  icon: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="mb-9" aria-labelledby={`settings-${sectionId(title)}`}>
      <h2
        className="flex items-center gap-2 border-b border-border/70 pb-2 text-sm font-semibold"
        id={`settings-${sectionId(title)}`}
      >
        <span className="text-muted-foreground [&>svg]:size-4">{icon}</span>
        {title}
      </h2>
      <div className="divide-y divide-border/60">{children}</div>
    </section>
  );
}

function SettingsRow({
  title,
  description,
  control,
}: {
  title: string;
  description: string;
  control: ReactNode;
}) {
  return (
    <div className="grid gap-3 py-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
      <div className="min-w-0">
        <h3 className="text-sm font-medium">{title}</h3>
        <p className="mt-0.5 break-words text-xs leading-5 text-muted-foreground">
          {description}
        </p>
      </div>
      <div className="justify-self-start sm:justify-self-end">{control}</div>
    </div>
  );
}

function ThemeControl({ theme }: { theme: ThemeName }) {
  return (
    <div className="flex rounded-lg bg-muted p-0.5" aria-label="Theme">
      <ThemeButton
        active={theme === "earl-grey-light"}
        icon={<SunIcon />}
        label="Light"
        theme="earl-grey-light"
      />
      <ThemeButton
        active={theme === "one-dark"}
        icon={<MoonIcon />}
        label="Dark"
        theme="one-dark"
      />
    </div>
  );
}

function ThemeButton({
  theme,
  label,
  icon,
  active,
}: {
  theme: ThemeName;
  label: string;
  icon: ReactNode;
  active: boolean;
}) {
  return (
    <button
      aria-pressed={active}
      className={cn(
        "flex min-h-11 items-center gap-1.5 rounded-md px-2.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring md:min-h-8",
        active
          ? "bg-background font-medium text-foreground shadow-xs"
          : "text-muted-foreground hover:text-foreground",
      )}
      onClick={() => useApp.getState().setTheme(theme)}
      type="button"
    >
      <span className="[&>svg]:size-3.5">{icon}</span>
      {label}
    </button>
  );
}

function reportError(message: string) {
  window.dispatchEvent(new CustomEvent("pecan:error", { detail: message }));
}

function sectionId(title: string) {
  return title.toLowerCase().replaceAll(" ", "-");
}

/** Font picker for UI or code text: curated stacks plus locally installed fonts. */
function FontControl({
  code,
  kind,
  onChange,
  title,
}: {
  code: string;
  kind: "ui" | "code";
  onChange: (font: string) => void;
  title: string;
}) {
  const [installed, setInstalled] = useState<string[]>([]);
  const [scanning, setScanning] = useState(false);
  const systemLabel = kind === "ui" ? "System default (SF Pro)" : "System default (SF Mono)";
  const curated = kind === "ui" ? CURATED_UI_FONTS : CURATED_CODE_FONTS;
  const options = [...curated, ...installed.filter((font) => !curated.includes(font))];
  const canScan = typeof (globalThis as { queryLocalFonts?: unknown }).queryLocalFonts === "function";

  async function scan() {
    setScanning(true);
    try {
      setInstalled(await loadInstalledFonts());
    } finally {
      setScanning(false);
    }
  }

  return (
    <SettingsRow
      control={
        <div className="flex flex-col items-stretch gap-1.5">
          <Select
            onValueChange={(next) => {
              if (!next) return;
              onChange(next);
            }}
            value={code}
          >
            <SelectTrigger
              aria-label={title}
              className="min-h-11 w-[min(17rem,calc(100vw-2.5rem))] sm:min-h-9 sm:w-64"
            >
              <SelectValue>
                <span
                  className="truncate"
                  style={{
                    fontFamily:
                      code === SYSTEM_UI || code === SYSTEM_CODE
                        ? undefined
                        : `"${code}", sans-serif`,
                  }}
                >
                  {code === SYSTEM_UI || code === SYSTEM_CODE ? systemLabel : code}
                </span>
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={kind === "ui" ? SYSTEM_UI : SYSTEM_CODE}>
                {systemLabel}
              </SelectItem>
              {options.map((font) => (
                <SelectItem key={font} value={font}>
                  <span style={{ fontFamily: `"${font}", sans-serif` }}>{font}</span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {canScan && installed.length === 0 ? (
            <Button
              className="self-start text-xs"
              disabled={scanning}
              onClick={() => void scan()}
              size="xs"
              variant="ghost"
            >
              {scanning ? "Scanning…" : "Scan installed fonts"}
            </Button>
          ) : null}
        </div>
      }
      description={
        kind === "ui"
          ? "Applies to interface text. Falls back to the system stack if missing."
          : "Applies to code blocks and inline code."
      }
      title={title}
    />
  );
}
