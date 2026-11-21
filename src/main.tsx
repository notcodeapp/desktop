import { createRoot } from 'react-dom/client';
import { Component, type ErrorInfo, type ReactNode } from 'react';
import App from './App';
class ErrorBoundary extends Component<{children:ReactNode},{error:string}> {
  state={error:''};
  static getDerivedStateFromError(error:Error){return{error:error.message};}
  componentDidCatch(error:Error,info:ErrorInfo){console.error(error,info.componentStack);}
  render(){if(this.state.error)return <div className="fatal"><h1>Не удалось открыть рабочую область</h1><pre>{this.state.error}</pre><button onClick={()=>window.location.reload()}>Перезапустить интерфейс</button><p>Несохранённые изменения могут быть потеряны. Диалоги сохраняются в локальной SQLite.</p></div>;return this.props.children;}
}
createRoot(document.getElementById('root')!).render(<ErrorBoundary><App/></ErrorBoundary>);
