// Test-only WPF app. It creates two controls and reports its own state; no input injection.
using System;
using System.IO;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Interop;

class HwndlessFixture {
    static Window window;
    static TextBox a, b;
    static string output;
    static int? aHandle, bHandle;
    static void Save() {
        if (!window.IsLoaded) return;
        var data = new { a = a.Text, b = b.Text,
            a_hwnd = aHandle, b_hwnd = bHandle,
            window = new WindowInteropHelper(window).Handle.ToInt64() };
        File.WriteAllText(output, new JavaScriptSerializer().Serialize(data));
    }
    [STAThread] static void Main(string[] args) {
        output = args[0];
        window = new Window { Title = "CUA HWND-less teaching test - disposable",
            Width = 760, Height = 360, Left = 160, Top = 160 };
        var panel = new StackPanel { Margin = new Thickness(20) };
        a = new TextBox { Height = 50, FontSize = 22 };
        b = new TextBox { Height = 50, FontSize = 22 };
        AutomationProperties.SetName(a, "Virtual field A");
        AutomationProperties.SetAutomationId(a, "virtual-a");
        AutomationProperties.SetName(b, "Virtual field B");
        AutomationProperties.SetAutomationId(b, "virtual-b");
        panel.Children.Add(new TextBlock { Text = "Virtual field A (no child HWND)" });
        panel.Children.Add(a);
        panel.Children.Add(new TextBlock { Text = "Virtual field B (no child HWND)", Margin = new Thickness(0, 25, 0, 0) });
        panel.Children.Add(b);
        window.Content = panel;
        window.ContentRendered += delegate {
            Save();
            var handle = new WindowInteropHelper(window).Handle;
            // Read-only UIA probe runs off the UI thread so the real WPF
            // provider can answer. Null means unverified; never assume zero.
            var probe = new Thread(delegate() {
                try {
                    var root = AutomationElement.FromHandle(handle);
                    var first = root.FindFirst(TreeScope.Descendants,
                        new PropertyCondition(AutomationElement.AutomationIdProperty, "virtual-a"));
                    var second = root.FindFirst(TreeScope.Descendants,
                        new PropertyCondition(AutomationElement.AutomationIdProperty, "virtual-b"));
                    int ah = first.Current.NativeWindowHandle, bh = second.Current.NativeWindowHandle;
                    window.Dispatcher.BeginInvoke(new Action(delegate { aHandle = ah; bHandle = bh; Save(); }));
                } catch (Exception error) { File.WriteAllText(output + ".error", error.ToString()); }
            });
            probe.IsBackground = true;
            probe.Start();
        };
        a.TextChanged += delegate { Save(); };
        b.TextChanged += delegate { Save(); };
        new Application().Run(window);
    }
}
