using System.Threading.Tasks;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Input.Platform;

using Avalonia.Interactivity;

namespace Everty.Desktop.Avalonia;

public partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
    }

    private async void OnStartHostClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            await viewModel.StartHostAsync();
        }
    }

    private async void OnStopHostClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            await viewModel.StopHostAsync();
        }
    }

    private async void OnRefreshHostClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            await viewModel.RefreshHostsAsync();
        }
    }

    private async void OnDemoLoginClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            await viewModel.LoginDemoAsync("admin@everty.local", "admin");
        }
    }

    private async void OnRefreshHostsClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            await viewModel.RefreshHostsAsync();
        }
    }

    private async void OnConnectByCodeClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            await viewModel.ConnectByCodeAsync();
        }
    }

    private async void OnRestoreClientClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            await viewModel.RestoreManagedSessionAsync();
        }
    }

    private async void OnStopClientClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            await viewModel.StopClientAsync();
        }
    }

    private void OnOpenClientPlaybackClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            viewModel.OpenClientPlaybackWindow();
        }
    }

    private void OnHideClientPlaybackClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            viewModel.HideClientPlaybackWindow();
        }
    }

    private void OnToggleDiagnosticsClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            viewModel.ToggleDiagnostics();
        }
    }

    private void OnToggleAdvancedClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            viewModel.ToggleAdvanced();
        }
    }

    private async void OnCopyDiagnosticsClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            var clipboard = TopLevel.GetTopLevel(this)?.Clipboard;
            if (clipboard is not null)
            {
                await clipboard.SetTextAsync(viewModel.DiagnosticsText);
            }
        }
    }

    private async void OnCopyHostCodeClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is MainWindowViewModel viewModel)
        {
            var code = viewModel.HostCode;
            if (string.IsNullOrWhiteSpace(code) || code == "-")
            {
                return;
            }

            var clipboard = TopLevel.GetTopLevel(this)?.Clipboard;
            if (clipboard is not null)
            {
                await clipboard.SetTextAsync(code);
            }
        }
    }

    private async void OnWindowKeyDown(object? sender, KeyEventArgs e)
    {
        if (DataContext is not MainWindowViewModel viewModel)
        {
            return;
        }

        if (e.Key == Key.C && e.KeyModifiers.HasFlag(KeyModifiers.Control) && e.KeyModifiers.HasFlag(KeyModifiers.Shift))
        {
            var code = viewModel.HostCode;
            if (!string.IsNullOrWhiteSpace(code) && code != "-")
            {
                var clipboard = TopLevel.GetTopLevel(this)?.Clipboard;
                if (clipboard is not null)
                {
                    await clipboard.SetTextAsync(code);
                    e.Handled = true;
                }
            }
        }
        else if (e.Key == Key.D && e.KeyModifiers.HasFlag(KeyModifiers.Control) && e.KeyModifiers.HasFlag(KeyModifiers.Shift))
        {
            var clipboard = TopLevel.GetTopLevel(this)?.Clipboard;
            if (clipboard is not null)
            {
                await clipboard.SetTextAsync(viewModel.DiagnosticsText);
                e.Handled = true;
            }
        }
        else if (e.Key == Key.Escape)
        {
            if (viewModel.DiagnosticsVisible)
            {
                viewModel.ToggleDiagnostics();
                e.Handled = true;
            }
            else if (viewModel.AdvancedVisible)
            {
                viewModel.ToggleAdvanced();
                e.Handled = true;
            }
        }
        else if (e.Key == Key.D1 && e.KeyModifiers.HasFlag(KeyModifiers.Control) && e.KeyModifiers.HasFlag(KeyModifiers.Shift))
        {
            viewModel.SelectedTabIndex = 0;
            e.Handled = true;
        }
        else if (e.Key == Key.D2 && e.KeyModifiers.HasFlag(KeyModifiers.Control) && e.KeyModifiers.HasFlag(KeyModifiers.Shift))
        {
            viewModel.SelectedTabIndex = 1;
            e.Handled = true;
        }
    }
}
