using System.Linq;
using System.Drawing;
using System.Windows.Forms;
using ReceiverNative;

namespace Everty.Desktop.Avalonia;

internal sealed class DesktopClientPlaybackForm : Form
{
    private readonly Panel _playbackHost = new()
    {
        Dock = DockStyle.Fill,
        BackColor = Color.Black,
        Margin = Padding.Empty,
    };

    private readonly Label _statusLabel = new()
    {
        Dock = DockStyle.Bottom,
        Height = 30,
        ForeColor = Color.Gainsboro,
        BackColor = Color.FromArgb(18, 22, 29),
        Padding = new Padding(10, 7, 10, 0),
        Text = "Idle",
    };

    private bool _fullscreen;
    private Rectangle _restoreBounds = Rectangle.Empty;
    private FormBorderStyle _restoreBorderStyle = FormBorderStyle.Sizable;
    private FormWindowState _restoreWindowState = FormWindowState.Normal;

    public DesktopClientPlaybackForm()
    {
        Text = "Everty Desktop Client";
        BackColor = Color.Black;
        MinimumSize = new Size(960, 600);
        Size = new Size(1280, 760);
        StartPosition = FormStartPosition.CenterScreen;
        KeyPreview = true;
        Controls.Add(_playbackHost);
        Controls.Add(_statusLabel);
    }

    public Control PlaybackHost => _playbackHost;

    public void ApplySnapshot(ReceiverSessionSnapshot snapshot)
    {
        Text = snapshot.Listening
            ? $"Everty Desktop Client · {snapshot.Status}"
            : "Everty Desktop Client";
        _statusLabel.Text = string.Join(" · ", new[]
        {
            snapshot.Status,
            snapshot.Codec,
            snapshot.Resolution,
            snapshot.RemoteEndpoint,
        }.Where(static value => !string.IsNullOrWhiteSpace(value) && value != "-"));
    }

    protected override bool ProcessCmdKey(ref Message msg, Keys keyData)
    {
        if (keyData == Keys.F11 || keyData == (Keys.Alt | Keys.Enter))
        {
            ToggleFullscreen();
            return true;
        }

        if (keyData == Keys.Escape && _fullscreen)
        {
            ToggleFullscreen();
            return true;
        }

        return base.ProcessCmdKey(ref msg, keyData);
    }

    private void ToggleFullscreen()
    {
        if (_fullscreen)
        {
            FormBorderStyle = _restoreBorderStyle;
            WindowState = _restoreWindowState;
            Bounds = _restoreBounds;
            _fullscreen = false;
            return;
        }

        _restoreBounds = Bounds;
        _restoreBorderStyle = FormBorderStyle;
        _restoreWindowState = WindowState;
        FormBorderStyle = FormBorderStyle.None;
        WindowState = FormWindowState.Normal;
        Bounds = Screen.FromControl(this).Bounds;
        _fullscreen = true;
    }
}
