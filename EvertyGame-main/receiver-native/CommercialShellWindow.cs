namespace ReceiverNative;

using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

internal sealed class CommercialShellWindow : Window
{
    private readonly TextBox _serverUrlBox = new()
    {
        Text = "http://46.45.217.19:5180",
        FontSize = 16,
        Padding = new Thickness(14, 10, 14, 10),
        BorderThickness = new Thickness(1),
        BorderBrush = new SolidColorBrush(Color.FromRgb(56, 63, 77)),
        Background = new SolidColorBrush(Color.FromRgb(15, 18, 24)),
        Foreground = Brushes.White,
        MinWidth = 320,
    };

    private readonly TextBlock _statusText = new()
    {
        Text = "Выбери режим. Для локального теста используй demo-учетки admin/admin или test/test.",
        Foreground = new SolidColorBrush(Color.FromRgb(168, 181, 202)),
        FontSize = 15,
        TextWrapping = TextWrapping.Wrap,
    };

    public CommercialShellWindow()
    {
        Title = "Everty Studio";
        Width = 1320;
        Height = 860;
        MinWidth = 1180;
        MinHeight = 760;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        Background = new SolidColorBrush(Color.FromRgb(7, 10, 15));
        Content = BuildContent();
    }

    private UIElement BuildContent()
    {
        var root = new Grid
        {
            Margin = new Thickness(28),
        };

        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });

        var hero = new StackPanel
        {
            Orientation = Orientation.Vertical,
            Margin = new Thickness(0, 0, 0, 28),
        };
        hero.Children.Add(new Border
        {
            Background = new SolidColorBrush(Color.FromArgb(36, 92, 144, 255)),
            BorderBrush = new SolidColorBrush(Color.FromArgb(90, 92, 144, 255)),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(999),
            Padding = new Thickness(14, 6, 14, 6),
            Child = new TextBlock
            {
                Text = "Everty Commercial Desktop",
                Foreground = Brushes.White,
                FontSize = 13,
                FontWeight = FontWeights.SemiBold,
            },
            HorizontalAlignment = HorizontalAlignment.Left,
        });
        hero.Children.Add(new TextBlock
        {
            Text = "Everty Studio",
            FontSize = 56,
            FontWeight = FontWeights.Bold,
            Foreground = Brushes.White,
            Margin = new Thickness(0, 18, 0, 8),
        });
        hero.Children.Add(new TextBlock
        {
            Text = "Игровой streaming для Windows и Android. Один входной экран, понятные режимы и локальный managed connect без operator-консоли.",
            FontSize = 18,
            Foreground = new SolidColorBrush(Color.FromRgb(176, 188, 206)),
            TextWrapping = TextWrapping.Wrap,
            MaxWidth = 920,
        });
        Grid.SetRow(hero, 0);
        root.Children.Add(hero);

        var serverCard = CreatePanel();
        serverCard.Margin = new Thickness(0, 0, 0, 22);
        serverCard.Child = new StackPanel
        {
            Children =
            {
                new TextBlock
                {
                    Text = "Сервер",
                    FontSize = 16,
                    FontWeight = FontWeights.SemiBold,
                    Foreground = Brushes.White,
                },
                new TextBlock
                {
                    Text = "Укажи control-plane URL. Этот адрес будет передан в host/client workspace.",
                    FontSize = 14,
                    Foreground = new SolidColorBrush(Color.FromRgb(156, 169, 190)),
                    Margin = new Thickness(0, 6, 0, 14),
                },
                _serverUrlBox,
                new TextBlock
                {
                    Text = "Demo auth: admin/admin и test/test",
                    FontSize = 13,
                    Foreground = new SolidColorBrush(Color.FromRgb(112, 226, 169)),
                    Margin = new Thickness(0, 10, 0, 0),
                },
            },
        };
        Grid.SetRow(serverCard, 1);
        root.Children.Add(serverCard);

        var statusCard = CreatePanel();
        statusCard.Margin = new Thickness(0, 0, 0, 22);
        statusCard.Child = _statusText;
        Grid.SetRow(statusCard, 2);
        root.Children.Add(statusCard);

        var modesGrid = new Grid();
        modesGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        modesGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        modesGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(320) });

        var hostCard = CreateModeCard(
            title: "Дать доступ к этому ПК",
            subtitle: "Хост-режим для игрового компьютера. Регистрирует ПК в control-plane, ждет клиента и стартует sender по managed lease.",
            actionText: "Открыть host workspace",
            accent: Color.FromRgb(84, 190, 255),
            role: AppRole.Send);
        Grid.SetColumn(hostCard, 0);
        modesGrid.Children.Add(hostCard);

        var clientCard = CreateModeCard(
            title: "Подключиться к другому ПК",
            subtitle: "Клиентский режим. Загружает список компьютеров, создает managed session и управляет reconnect/stop/start.",
            actionText: "Открыть client workspace",
            accent: Color.FromRgb(110, 108, 255),
            role: AppRole.Receive);
        Grid.SetColumn(clientCard, 1);
        modesGrid.Children.Add(clientCard);

        var notes = CreatePanel();
        notes.Margin = new Thickness(18, 0, 0, 0);
        notes.Child = new StackPanel
        {
            Children =
            {
                new TextBlock
                {
                    Text = "Что дальше",
                    FontSize = 16,
                    FontWeight = FontWeights.SemiBold,
                    Foreground = Brushes.White,
                },
                CreateBullet("1. На игровом ПК открой host workspace."),
                CreateBullet("2. Войди demo-учеткой и дождись статуса «ПК доступен»."),
                CreateBullet("3. На телефоне войди той же учеткой и выбери этот ПК."),
                CreateBullet("4. Для ручной диагностики legacy sender controls остаются в Advanced."),
            },
        };
        Grid.SetColumn(notes, 2);
        modesGrid.Children.Add(notes);

        Grid.SetRow(modesGrid, 3);
        root.Children.Add(modesGrid);

        return root;
    }

    private Border CreateModeCard(string title, string subtitle, string actionText, Color accent, AppRole role)
    {
        var card = CreatePanel();
        card.Margin = role == AppRole.Send ? new Thickness(0, 0, 9, 0) : new Thickness(9, 0, 0, 0);
        card.BorderBrush = new SolidColorBrush(Color.FromArgb(82, accent.R, accent.G, accent.B));

        var openButton = new Button
        {
            Content = actionText,
            FontSize = 15,
            FontWeight = FontWeights.SemiBold,
            Padding = new Thickness(18, 12, 18, 12),
            Background = new SolidColorBrush(accent),
            Foreground = Brushes.White,
            BorderThickness = new Thickness(0),
            Cursor = System.Windows.Input.Cursors.Hand,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        openButton.Click += (_, _) => LaunchWorkspace(role);

        card.Child = new StackPanel
        {
            Children =
            {
                new Border
                {
                    Background = new SolidColorBrush(Color.FromArgb(36, accent.R, accent.G, accent.B)),
                    BorderBrush = new SolidColorBrush(Color.FromArgb(82, accent.R, accent.G, accent.B)),
                    BorderThickness = new Thickness(1),
                    CornerRadius = new CornerRadius(999),
                    Padding = new Thickness(12, 6, 12, 6),
                    Child = new TextBlock
                    {
                        Text = role == AppRole.Send ? "Host mode" : "Client mode",
                        Foreground = Brushes.White,
                        FontSize = 13,
                        FontWeight = FontWeights.SemiBold,
                    },
                    HorizontalAlignment = HorizontalAlignment.Left,
                },
                new TextBlock
                {
                    Text = title,
                    FontSize = 28,
                    FontWeight = FontWeights.Bold,
                    Foreground = Brushes.White,
                    Margin = new Thickness(0, 18, 0, 10),
                    TextWrapping = TextWrapping.Wrap,
                },
                new TextBlock
                {
                    Text = subtitle,
                    FontSize = 15,
                    Foreground = new SolidColorBrush(Color.FromRgb(176, 188, 206)),
                    TextWrapping = TextWrapping.Wrap,
                    Margin = new Thickness(0, 0, 0, 22),
                },
                openButton,
            },
        };

        return card;
    }

    private Border CreatePanel() => new()
    {
        Background = new SolidColorBrush(Color.FromRgb(14, 18, 25)),
        BorderBrush = new SolidColorBrush(Color.FromRgb(33, 39, 51)),
        BorderThickness = new Thickness(1),
        CornerRadius = new CornerRadius(24),
        Padding = new Thickness(24),
    };

    private TextBlock CreateBullet(string text) => new()
    {
        Text = text,
        FontSize = 14,
        Foreground = new SolidColorBrush(Color.FromRgb(176, 188, 206)),
        Margin = new Thickness(0, 10, 0, 0),
        TextWrapping = TextWrapping.Wrap,
    };

    private void LaunchWorkspace(AppRole role)
    {
        var baseUrl = _serverUrlBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(baseUrl))
        {
            _statusText.Text = "Сначала укажи адрес сервера.";
            return;
        }

        _statusText.Text = role == AppRole.Send
            ? "Открываю host workspace. На игровом ПК оставь его запущенным."
            : "Открываю client workspace. Выбери ПК и запускай managed session.";

        var workspace = new MainForm(new MainFormLaunchOptions(
            InitialRole: role,
            ControlPlaneUrl: baseUrl,
            AdvancedMode: false,
            LockRoleSelection: true));

        workspace.FormClosed += (_, _) =>
        {
            Dispatcher.Invoke(() =>
            {
                Show();
                Activate();
                _statusText.Text = "Workspace закрыт. Можно открыть host или client режим снова.";
            });
        };

        Hide();
        workspace.Show();
    }
}
