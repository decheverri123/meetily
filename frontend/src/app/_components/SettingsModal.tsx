import { ModelConfig } from "@/components/ModelSettingsModal";
import { PreferenceSettings } from "@/components/PreferenceSettings";
import { DeviceSelection } from "@/components/DeviceSelection";
import { LanguageSelection } from "@/components/LanguageSelection";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { toast } from "sonner";
import { useConfig } from "@/contexts/ConfigContext";
import { useRecordingState } from "@/contexts/RecordingStateContext";

type modalType = "modelSettings" | "deviceSettings" | "languageSettings" | "modelSelector" | "errorAlert" | "chunkDropWarning";

/**
 * SettingsModals Component
 *
 * All settings modals consolidated into a single component.
 * Uses ConfigContext and RecordingStateContext internally - no prop drilling needed!
 */

interface SettingsModalsProps {
  modals: {
    modelSettings: boolean;
    deviceSettings: boolean;
    languageSettings: boolean;
    modelSelector: boolean;
    errorAlert: boolean;
    chunkDropWarning: boolean;
  };
  messages: {
    errorAlert: string;
    chunkDropWarning: string;
    modelSelector: string;
  };
  onClose: (name: modalType) => void;
}

export function SettingsModals({
  modals,
  messages,
  onClose,
}: SettingsModalsProps) {
  // Contexts
  const {
    modelConfig,
    setModelConfig,
    models,
    modelOptions,
    error,
    selectedDevices,
    setSelectedDevices,
    selectedLanguage,
    setSelectedLanguage,
    transcriptModelConfig,
    setTranscriptModelConfig,
    showConfidenceIndicator,
    toggleConfidenceIndicator,
  } = useConfig();

  const { isRecording } = useRecordingState();

  return <>
    {/* Legacy Settings Modal */}
    {modals.modelSettings && (
      <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center z-50 p-4">
        <div className="glass-panel shadow-xl shadow-black/20 max-w-4xl w-full max-h-[90vh] overflow-hidden flex flex-col">
          {/* Header */}
          <div className="flex justify-between items-center p-6 border-b border-border/10">
            <h3 className="text-xl font-semibold text-foreground">Preferences</h3>
            <button
              onClick={() => onClose("modelSettings")
              }
              className="text-muted-foreground hover:text-foreground"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          {/* Content - Scrollable */}
          <div className="flex-1 overflow-y-auto p-6 space-y-8">
            {/* General Preferences Section */}
            <PreferenceSettings />

            {/* Divider */}
            <div className="border-t border-border/10 pt-8">
              <h4 className="text-lg font-semibold text-foreground mb-4">AI Model Configuration</h4>
              <div className="space-y-4">
                <div>
                  <label className="block text-sm font-medium text-foreground/80 mb-1">
                    Summarization Model
                  </label>
                  <div className="flex space-x-2">
                    <select
                      className="px-3 py-2 text-sm text-foreground bg-secondary/5 border border-border/10 rounded-md focus:outline-none focus:ring-1 focus:ring-primary focus:border-primary [&>option]:bg-background [&>option]:text-foreground"
                      value={modelConfig.provider}
                      onChange={(e) => {
                        const provider = e.target.value as ModelConfig['provider'];
                        setModelConfig({
                          ...modelConfig,
                          provider,
                          model: modelOptions[provider][0]
                        });
                      }}
                    >
                      <option value="builtin-ai">Built-in AI</option>
                      <option value="claude">Claude</option>
                      <option value="groq">Groq</option>
                      <option value="ollama">Ollama</option>
                      <option value="openrouter">OpenRouter</option>
                      <option value="openai">OpenAI</option>
                    </select>

                    <select
                      className="flex-1 px-3 py-2 text-sm text-foreground bg-secondary/5 border border-border/10 rounded-md focus:outline-none focus:ring-1 focus:ring-primary focus:border-primary [&>option]:bg-background [&>option]:text-foreground"
                      value={modelConfig.model}
                      onChange={(e) => setModelConfig((prev: ModelConfig) => ({ ...prev, model: e.target.value }))}
                    >
                      {modelOptions[modelConfig.provider].map((model: string) => (
                        <option key={model} value={model}>
                          {model}
                        </option>
                      ))}
                    </select>
                  </div>
                </div>
                {modelConfig.provider === 'ollama' && (
                  <div>
                    <h4 className="text-lg font-bold mb-4 text-foreground">Available Ollama Models</h4>
                    {error && (
                      <div className="bg-destructive/15 border border-destructive/30 text-destructive px-4 py-3 rounded mb-4">
                        {error}
                      </div>
                    )}
                    <div className="grid gap-4 max-h-[400px] overflow-y-auto pr-2">
                      {models.map((model) => (
                        <div
                          key={model.id}
                          className={`glass-card p-4 cursor-pointer transition-colors ${modelConfig.model === model.name ? 'ring-2 ring-primary bg-primary/10' : 'hover:bg-secondary/10'
                            }`}
                          onClick={() => setModelConfig((prev: ModelConfig) => ({ ...prev, model: model.name }))}
                        >
                          <h3 className="font-bold text-foreground">{model.name}</h3>
                          <p className="text-foreground/80">Size: {model.size}</p>
                          <p className="text-foreground/80">Modified: {model.modified}</p>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* Footer */}
          <div className="border-t border-border/10 p-6 flex justify-end">
            <button
              onClick={() => onClose('modelSettings')}
              className="px-4 py-2 text-sm font-medium text-background bg-primary rounded-md hover:bg-primary/90 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-primary focus:ring-offset-background"
            >
              Done
            </button>
          </div>
        </div>
      </div>
    )}

    {/* Device Settings Modal */}
    {modals.deviceSettings && (
      <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center z-50">
        <div className="glass-panel p-6 max-w-md w-full mx-4 shadow-xl shadow-black/20">
          <div className="flex justify-between items-center mb-4">
            <h3 className="text-lg font-semibold text-foreground">Audio Device Settings</h3>
            <button
              onClick={() => onClose('deviceSettings')}
              className="text-muted-foreground hover:text-foreground"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          <DeviceSelection
            selectedDevices={selectedDevices}
            onDeviceChange={setSelectedDevices}
            disabled={isRecording}
          />

          <div className="mt-6 flex justify-end">
            <button
              onClick={() => {
                const micDevice = selectedDevices.micDevice || 'Default';
                const systemDevice = selectedDevices.systemDevice || 'Default';
                toast.success("Devices selected", {
                  description: `Microphone: ${micDevice}, System Audio: ${systemDevice}`
                });
                onClose('deviceSettings');
              }}
              className="px-4 py-2 text-sm font-medium text-background bg-primary rounded-md hover:bg-primary/90 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-primary focus:ring-offset-background"
            >
              Done
            </button>
          </div>
        </div>
      </div>
    )}

    {/* Language Settings Modal */}
    {modals.languageSettings && (
      <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center z-50">
        <div className="glass-panel p-6 max-w-md w-full mx-4 shadow-xl shadow-black/20">
          <div className="flex justify-between items-center mb-4">
            <h3 className="text-lg font-semibold text-foreground">Language Settings</h3>
            <button
              onClick={() => onClose('languageSettings')}
              className="text-muted-foreground hover:text-foreground"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          <LanguageSelection
            selectedLanguage={selectedLanguage}
            onLanguageChange={setSelectedLanguage}
            disabled={isRecording}
            provider={transcriptModelConfig.provider}
          />

          <div className="mt-6 flex justify-end">
            <button
              onClick={() => onClose('languageSettings')}
              className="px-4 py-2 text-sm font-medium text-background bg-primary rounded-md hover:bg-primary/90 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-primary focus:ring-offset-background"
            >
              Done
            </button>
          </div>
        </div>
      </div>
    )}

    {/* Model Selection Modal */}
    {modals.modelSelector && (
      <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center z-50">
        <div className="glass-panel max-w-4xl w-full mx-4 shadow-xl shadow-black/20 max-h-[90vh] flex flex-col">
          {/* Fixed Header */}
          <div className="flex justify-between items-center p-6 pb-4 border-b border-border/10">
            <h3 className="text-lg font-semibold text-foreground">
              {messages.modelSelector ? 'Speech Recognition Setup Required' : 'Transcription Model Settings'}
            </h3>
            <button
              onClick={() => onClose('modelSelector')}
              className="text-muted-foreground hover:text-foreground"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          {/* Scrollable Content */}
          <div className="flex-1 overflow-y-auto p-6 pt-4">
            <TranscriptSettings
              transcriptModelConfig={transcriptModelConfig}
              setTranscriptModelConfig={setTranscriptModelConfig}
              onModelSelect={() => onClose('modelSelector')}
            />
          </div>

          {/* Fixed Footer */}
          <div className="p-6 pt-4 border-t border-border/10 flex items-center justify-between">
            {/* Confidence Indicator Toggle */}
            <div className="flex items-center gap-3">
              <label className="relative inline-flex items-center cursor-pointer">
                <input
                  type="checkbox"
                  checked={showConfidenceIndicator}
                  onChange={(e) => toggleConfidenceIndicator(e.target.checked)}
                  className="sr-only peer"
                />
                <div className="w-11 h-6 bg-secondary/15 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/40 rounded-full peer peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-border/20 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
              </label>
              <div>
                <p className="text-sm font-medium text-foreground/80">Show Confidence Indicators</p>
                <p className="text-xs text-muted-foreground">Display colored dots showing transcription confidence quality</p>
              </div>
            </div>

            <button
              onClick={() => onClose('modelSelector')}
              className="px-4 py-2 text-sm font-medium text-foreground border border-border/10 bg-secondary/5 rounded-md hover:bg-secondary/10 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring/60 focus:ring-offset-background"
            >
              {messages.modelSelector ? 'Cancel' : 'Done'}
            </button>
          </div>
        </div>
      </div>
    )}

    {/* Error Alert Modal */}
    {modals.errorAlert && (
      <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center z-50">
        <Alert className="max-w-md mx-4 glass-panel border-destructive/30 bg-destructive/10 shadow-xl shadow-black/20">
          <AlertTitle className="text-destructive">Recording Stopped</AlertTitle>
          <AlertDescription className="text-foreground/80">
            {messages.errorAlert}
            <button
              onClick={() => onClose('errorAlert')}
              className="ml-2 text-destructive hover:text-destructive/80 underline"
            >
              Dismiss
            </button>
          </AlertDescription>
        </Alert>
      </div>
    )}

    {/* Chunk Drop Warning Modal */}
    {modals.chunkDropWarning && (
      <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center z-50">
        <Alert className="max-w-lg mx-4 glass-panel border-amber-400/30 bg-amber-500/10 shadow-xl shadow-black/20">
          <AlertTitle className="text-amber-400">Transcription Performance Warning</AlertTitle>
          <AlertDescription className="text-foreground/80">
            {messages.chunkDropWarning}
            <button
              onClick={() => onClose('chunkDropWarning')}
              className="ml-2 text-amber-400 hover:text-amber-300 underline"
            >
              Dismiss
            </button>
          </AlertDescription>
        </Alert>
      </div>
    )}
  </>
}
