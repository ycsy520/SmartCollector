// 错误边界：React 规定边界必须是 class。被包裹的子树**渲染期**抛异常时，接住并显示降级 UI，
// 而不是让整棵根树崩成白/黑屏（本项目此前无边界：任一组件 render 抛错 = 全屏死、ESC 失效）。
// 边界只覆盖 render/生命周期里的同步抛错，事件回调与异步里的错不在此列（那是另一类，别指望它兜）。
import { Component, type ErrorInfo, type ReactNode } from "react";
import { Icon } from "./icon";

interface Props {
  children: ReactNode;
  /** 变化即自动清除错误态：切页 / 换片段后不再卡在降级界面（详情页尤需）。 */
  resetKey?: unknown;
  /** 出错区域的语境名，用于降级标题（如「详情页」）；顶层用法传 reloadOnRetry 而非此。 */
  context?: string;
  /** 顶层边界：整个应用都崩了，重试只能是重新加载页面，而非就地重挂载。 */
  reloadOnRetry?: boolean;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // 只记控制台，不在界面透传堆栈；本项目无独立前端日志面。
    console.error("ErrorBoundary caught:", error, info.componentStack);
  }

  componentDidUpdate(prev: Props) {
    // resetKey 变了（如 detailId / page 切换）且此前是错误态 → 清空，让 children 重新挂载。
    if (this.state.error && prev.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  private retry = () => {
    if (this.props.reloadOnRetry) {
      window.location.reload();
      return;
    }
    this.setState({ error: null });
  };

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    const title = this.props.reloadOnRetry ? "应用出错了" : `${this.props.context ?? "这里"}出错了`;
    return (
      <div className="flex h-full min-h-0 w-full items-center justify-center overflow-auto p-6">
        <div className="md-elev w-full max-w-sm space-y-3 rounded-2xl bg-card p-5 text-center">
          <Icon name="TriangleAlert" size={28} className="mx-auto text-error" />
          <h2 className="text-sm font-semibold text-on-surface">{title}</h2>
          <p className="text-xs leading-relaxed text-on-surface-variant">
            这部分界面渲染失败，不影响其余功能。{this.props.reloadOnRetry ? "重新加载即可恢复。" : "可点重试；反复出现请切换页面或重启应用。"}
          </p>
          {/* 展示我们自身代码抛出的 message，便于凭截图定位；LLM 原始输出从不走到这里。 */}
          <p className="break-words font-mono text-[11px] text-hint">{error.message}</p>
          <button
            onClick={this.retry}
            className="md-press mx-auto mt-1 inline-flex items-center gap-1.5 rounded-full bg-secondary-container px-4 py-2 text-xs font-medium text-on-secondary-container"
          >
            <Icon name="RefreshCw" size={14} />
            {this.props.reloadOnRetry ? "重新加载" : "重试"}
          </button>
        </div>
      </div>
    );
  }
}
