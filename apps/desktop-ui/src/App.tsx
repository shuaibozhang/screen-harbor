import * as Switch from "@radix-ui/react-switch";
import { listen } from "@tauri-apps/api/event";
import {
  ChevronDown,
  ChevronRight,
  Clock3,
  Folder,
  Mic2,
  MonitorSpeaker,
  Settings2,
  Video,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useAppStore } from "./store";
import type { FinalizationEvent, RecordingState } from "./domain";
import "./styles.css";

function Toggle({ checked, label, disabled = false }: { checked: boolean; label: string; disabled?: boolean }) {
  return (
    <Switch.Root className="switch" defaultChecked={checked} disabled={disabled} aria-label={label}>
      <Switch.Thumb className="switch-thumb" />
    </Switch.Root>
  );
}

function StatusDot({ available }: { available: boolean }) {
  return <span className={available ? "status-dot ready" : "status-dot"} aria-hidden="true" />;
}

export function App() {
  const { snapshot, loading, error, hydrate, startSelection } = useAppStore();
  const [finalizationError, setFinalizationError] = useState<string | null>(null);

  useEffect(() => {
    void hydrate();
    let unlistenState: (() => void) | undefined;
    let unlistenFinalization: (() => void) | undefined;
    let disposed = false;
    void listen<RecordingState>("recording://state-changed", (event) => {
      if (event.payload === "completed" || event.payload === "idle") {
        void hydrate();
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenState = dispose;
    });
    void listen<FinalizationEvent>("recording://finalized", (event) => {
      if (!event.payload.success) {
        setFinalizationError(event.payload.error ?? "录制文件保存失败。");
      } else {
        setFinalizationError(null);
      }
      void hydrate();
    }).then((dispose) => {
      if (disposed) dispose();
      else unlistenFinalization = dispose;
    });
    return () => {
      disposed = true;
      unlistenState?.();
      unlistenFinalization?.();
    };
  }, [hydrate]);

  if (loading || !snapshot) {
    return <main className="shell loading">正在准备 FrameFlow…</main>;
  }

  return (
    <main className="shell">
      <header className="titlebar">
        <div className="brand"><span className="brand-mark" />FrameFlow</div>
        <button className="icon-button" aria-label="打开设置"><Settings2 size={18} /></button>
      </header>

      <div className="content">
        {(error || finalizationError) && (
          <div className="error" role="alert">{error ?? finalizationError}</div>
        )}

        <section className="hero" aria-labelledby="hero-title">
          <div>
            <p className="eyebrow">准备就绪</p>
            <h1 id="hero-title">录下重点，同时获得两个版本</h1>
            <p>轻量版快速分享，高清版安心留存。</p>
          </div>
          <div className="start-area">
            <div className="split-button">
              <button className="start-button" onClick={() => void startSelection()}>
                <span className="record-dot" />开始录屏
              </button>
              <button className="start-menu" aria-label="选择录制方式"><ChevronDown size={18} /></button>
            </div>
            <small>区域录制 · Alt + Shift + R</small>
          </div>
        </section>

        <section className="section" aria-labelledby="save-heading">
          <div className="section-heading"><h2 id="save-heading">保存位置</h2><button>更改</button></div>
          <button className="row path-row">
            <span className="row-icon"><Folder size={19} /></span>
            <span className="row-copy"><strong>{snapshot.saveDirectory}</strong><small>后续录制会按日期整理</small></span>
            <ChevronRight size={18} />
          </button>
        </section>

        <section className="section" aria-labelledby="input-heading">
          <div className="section-heading"><h2 id="input-heading">录制输入</h2></div>
          <div className="rows">
            <div className="row">
              <span className="row-icon"><Mic2 size={19} /></span>
              <span className="row-copy"><strong>麦克风</strong><small><StatusDot available={snapshot.microphone.available} />{snapshot.microphone.deviceName}</small></span>
              <Toggle checked={snapshot.microphone.enabled} disabled={!snapshot.microphone.available} label="开启麦克风" />
            </div>
            <div className="row">
              <span className="row-icon"><MonitorSpeaker size={19} /></span>
              <span className="row-copy"><strong>系统声音</strong><small><StatusDot available={snapshot.systemAudio.available} />录制设备播放的声音</small></span>
              <Toggle checked={snapshot.systemAudio.enabled} disabled={!snapshot.systemAudio.available} label="开启系统声音" />
            </div>
          </div>
        </section>

        <section className="section output" aria-labelledby="output-heading">
          <div className="section-heading"><h2 id="output-heading">输出</h2><button>调整</button></div>
          <div className="row">
            <span className="row-icon"><Video size={19} /></span>
            <span className="row-copy"><strong>高清母版与轻量预览</strong><small>{snapshot.outputSummary}</small></span>
          </div>
        </section>

        <section className="section recent" aria-labelledby="recent-heading">
          <div className="section-heading"><h2 id="recent-heading">最近录制</h2>{snapshot.recentRecordings.length > 0 && <button>查看全部</button>}</div>
          {snapshot.recentRecordings.length === 0 ? (
            <div className="empty"><Clock3 size={20} /><span><strong>还没有录制</strong><small>完成的内容会出现在这里</small></span></div>
          ) : (
            <div className="rows recent-list">
              {snapshot.recentRecordings.map((recording) => (
                <div className="row" key={recording.id}>
                  <span className="row-icon"><Video size={19} /></span>
                  <span className="row-copy">
                    <strong>{recording.name}</strong>
                    <small>{recording.createdLabel} · {recording.durationSeconds} 秒 · MP4 已保存</small>
                  </span>
                </div>
              ))}
            </div>
          )}
        </section>
      </div>
    </main>
  );
}
