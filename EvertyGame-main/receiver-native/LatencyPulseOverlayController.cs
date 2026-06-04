using System.Drawing.Drawing2D;

namespace ReceiverNative;

internal sealed class LatencyPulseOverlayController : IDisposable
{
    private readonly object _sync = new();
    private readonly ManualResetEventSlim _ready = new(false);

    private ApplicationContext? _applicationContext;
    private Thread? _uiThread;
    private PulseOverlayForm? _form;
    private bool _disposed;

    public void Flash(Rectangle monitorBounds)
    {
        EnsureInitialized();
        var form = _form;
        if (form is null || form.IsDisposed)
        {
            return;
        }

        try
        {
            form.BeginInvoke(new Action(() => form.Flash(monitorBounds)));
        }
        catch
        {
        }
    }

    public void HidePulse()
    {
        var form = _form;
        if (form is null || form.IsDisposed)
        {
            return;
        }

        try
        {
            form.BeginInvoke(new Action(form.HidePulse));
        }
        catch
        {
        }
    }

    public void Dispose()
    {
        lock (_sync)
        {
            if (_disposed)
            {
                return;
            }

            _disposed = true;
        }

        HidePulse();

        var form = _form;
        if (form is not null && !form.IsDisposed)
        {
            try
            {
                form.BeginInvoke(new Action(form.Close));
            }
            catch
            {
            }
        }

        var applicationContext = _applicationContext;
        if (applicationContext is not null)
        {
            try
            {
                applicationContext.MainForm?.BeginInvoke(new Action(applicationContext.ExitThread));
            }
            catch
            {
            }
        }

        if (_uiThread is not null && _uiThread.IsAlive)
        {
            _uiThread.Join(700);
        }

        _ready.Dispose();
    }

    private void EnsureInitialized()
    {
        lock (_sync)
        {
            if (_disposed || _uiThread is not null)
            {
                return;
            }

            _ready.Reset();
            _uiThread = new Thread(UiThreadMain)
            {
                IsBackground = true,
                Name = "EvertyLatencyPulseOverlay",
            };
            _uiThread.SetApartmentState(ApartmentState.STA);
            _uiThread.Start();
        }

        _ready.Wait(1500);
    }

    private void UiThreadMain()
    {
        Application.SetCompatibleTextRenderingDefault(false);
        using var form = new PulseOverlayForm();
        using var context = new ApplicationContext(form);
        _applicationContext = context;
        _form = form;
        _ = form.Handle;
        _ready.Set();
        Application.Run(context);
        _form = null;
        _applicationContext = null;
    }

    private sealed class PulseOverlayForm : Form
    {
        private readonly System.Windows.Forms.Timer _hideTimer;

        public PulseOverlayForm()
        {
            AutoScaleMode = AutoScaleMode.None;
            BackColor = Color.White;
            DoubleBuffered = true;
            FormBorderStyle = FormBorderStyle.None;
            ShowInTaskbar = false;
            StartPosition = FormStartPosition.Manual;
            TopMost = true;
            Visible = false;

            _hideTimer = new System.Windows.Forms.Timer
            {
                Interval = 150,
            };
            _hideTimer.Tick += static (sender, _) =>
            {
                if (sender is System.Windows.Forms.Timer timer && timer.Tag is PulseOverlayForm form)
                {
                    form.HidePulse();
                }
            };
            _hideTimer.Tag = this;
        }

        protected override bool ShowWithoutActivation => true;

        protected override CreateParams CreateParams
        {
            get
            {
                const int WsExToolWindow = 0x00000080;
                const int WsExTopmost = 0x00000008;
                const int WsExNoActivate = 0x08000000;

                var cp = base.CreateParams;
                cp.ExStyle |= WsExToolWindow | WsExTopmost | WsExNoActivate;
                return cp;
            }
        }

        public void Flash(Rectangle monitorBounds)
        {
            var squareSize = Math.Max(120, Math.Min(monitorBounds.Width, monitorBounds.Height) / 8);
            Bounds = new Rectangle(monitorBounds.Left + 32, monitorBounds.Top + 32, squareSize, squareSize);
            Show();
            TopMost = true;
            Invalidate();
            _hideTimer.Stop();
            _hideTimer.Start();
        }

        public void HidePulse()
        {
            _hideTimer.Stop();
            Hide();
        }

        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
            using var fillBrush = new SolidBrush(Color.FromArgb(245, 255, 255, 255));
            using var accentBrush = new SolidBrush(Color.FromArgb(255, 0, 230, 118));
            using var borderPen = new Pen(Color.Black, 6);
            using var innerPen = new Pen(Color.Black, 4);

            e.Graphics.FillRectangle(fillBrush, ClientRectangle);
            e.Graphics.DrawRectangle(borderPen, 3, 3, Width - 7, Height - 7);

            var innerRect = new Rectangle(Width / 5, Height / 5, Width * 3 / 5, Height * 3 / 5);
            e.Graphics.FillEllipse(accentBrush, innerRect);
            e.Graphics.DrawLine(innerPen, Width / 2, 18, Width / 2, Height - 18);
            e.Graphics.DrawLine(innerPen, 18, Height / 2, Width - 18, Height / 2);
        }
    }
}
