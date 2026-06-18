; ==========================================================================
;  NTEP Launcher Installer
;
;  这个脚本只负责把 CI 已经准备好的 Windows 包目录做成安装器。
;  它不下载运行库、不联网检查版本、不处理 deploy/config，也不修改后端文件。
;
;  Build with:
;    ISCC.exe install.iss /DTargetArch=x64 /DAppVersion=1.2.3
;    ISCC.exe install.iss /DTargetArch=arm64 /DAppVersion=1.2.3
; ==========================================================================

; --------------------------------------------------------------------------
;  编译期常量
;  CI 会通过 /D 参数覆盖 AppVersion 和 TargetArch。
;  SourceDir 默认指向 workflow 里生成的 package-root\NTEPilot。
; --------------------------------------------------------------------------
#define AppName "NTEP Launcher"
#define AppPublisher "NTEPilot"
#define AppExeName "ntep-launcher.exe"
#define AppRootName "NTEPilot"

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#ifndef TargetArch
  #define TargetArch "x64"
#endif

#ifndef SourceDir
  #define SourceDir "package-root\NTEPilot"
#endif

; --------------------------------------------------------------------------
;  目标架构
;  x64 包：允许 x64compatible，也就是 x64 Windows，以及能运行 x64 的
;          Windows on Arm。
;  arm64 包：只允许原生 Arm64 系统。
;
;  两个安装包的文件内容由 CI 当前 runner 决定：
;  windows-latest 生成 x64，windows-11-arm 生成 arm64。
; --------------------------------------------------------------------------
#if TargetArch == "arm64"
  #define TargetArchitecture "arm64"
#else
  #define TargetArchitecture "x64compatible"
#endif

; --------------------------------------------------------------------------
;  Setup 基本信息
;  OutputBaseFilename 会把架构写入文件名：
;  NTEP-Launcher-Windows-x64-Setup.exe
;  NTEP-Launcher-Windows-arm64-Setup.exe
; --------------------------------------------------------------------------
[Setup]
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppId={{091c8209-f403-454d-94f7-2bb1d72803c0}

DefaultDirName={autopf}\{#AppRootName}
AppendDefaultDirName=no
DefaultGroupName={#AppName}
OutputDir=installer-output
OutputBaseFilename=NTEP-Launcher-Windows-{#TargetArch}-Setup
SetupIconFile=icons\icon.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=no
MinVersion=10.0
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed={#TargetArchitecture}
ArchitecturesInstallIn64BitMode={#TargetArchitecture}
UninstallDisplayIcon={app}\{#AppExeName}

; 目前只启用简体中文安装器界面。
[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

; 可选桌面快捷方式，默认不勾选。
[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

; 给安装目录和运行时目录普通用户可写权限。
; 启动器会在 .venv/log 下写入运行环境和日志，因此不能只读。
[Dirs]
Name: "{app}"; Permissions: users-modify
Name: "{app}\.venv"; Permissions: users-modify
Name: "{app}\log"; Permissions: users-modify

; 安装整个 package-root\NTEPilot 目录。
; CI 已经把后端源码、.venv、uv、启动器 exe 都放在这里。
[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs; Permissions: users-modify

; 开始菜单和桌面快捷方式都指向启动器 exe，WorkingDir 必须是 {app}，
; 因为启动器会从当前工作区识别 main.py / pyproject.toml / uv.lock。
[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

; 安装完成页面的“启动”复选项。
[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent

[Code]
// --------------------------------------------------------------------------
//  路径工具
//  这些函数用于判断用户选择的目录是否危险。
//  我们允许安装到例如 D:\Apps\NTEPilot，但不允许装到：
//  - C:\Windows
//  - C:\Program Files 根目录本身
//  - D:\ 这种磁盘根目录
// --------------------------------------------------------------------------
function CanonicalDir(const Path: String): String;
begin
  Result := RemoveBackslashUnlessRoot(Path);
end;

function StartsWithPath(const Full, Prefix: String): Boolean;
var
  F, P: String;
begin
  F := CanonicalDir(Full);
  P := CanonicalDir(Prefix);
  Result :=
    (CompareText(F, P) = 0) or
    (
      (Length(F) > Length(P)) and
      (CompareText(Copy(F, 1, Length(P)), P) = 0) and
      (F[Length(P) + 1] = '\')
    );
end;

function IsDriveRootPath(const Path: String): Boolean;
var
  P: String;
begin
  P := AddBackslash(CanonicalDir(Path));
  Result := (Length(P) = 3) and (P[2] = ':') and (P[3] = '\');
end;

function IsBareProgramFilesPath(const Path: String): Boolean;
var
  P: String;
begin
  P := CanonicalDir(Path);
  Result :=
    (CompareText(P, CanonicalDir(ExpandConstant('{autopf}'))) = 0) or
    (CompareText(P, CanonicalDir(ExpandConstant('{commonpf32}'))) = 0);
  if Result then
    Exit;
  if IsWin64 then
    Result := CompareText(P, CanonicalDir(ExpandConstant('{commonpf64}'))) = 0;
end;

function IsUnderWindowsPath(const Path: String): Boolean;
begin
  Result := StartsWithPath(Path, ExpandConstant('{win}'));
end;

function IsUnsafeInstallPath(const Path: String): Boolean;
begin
  Result := IsUnderWindowsPath(Path) or IsBareProgramFilesPath(Path) or IsDriveRootPath(Path);
end;

// 如果用户只选了 Program Files 或磁盘根目录，不直接报错，
// 而是自动补上 NTEPilot 子目录，让向导停留在目录选择页给用户确认。
function AddAppRootIfBareContainer(const Dir: String): String;
var
  D: String;
begin
  D := CanonicalDir(Dir);
  Result := D;
  if IsBareProgramFilesPath(D) or IsDriveRootPath(D) then
    Result := AddBackslash(D) + '{#AppRootName}';
end;

// 用户点击“下一步”时检查安装目录。
// Windows 系统目录直接禁止；裸容器目录自动补成 NTEPilot 子目录。
function NextButtonClick(CurPageID: Integer): Boolean;
var
  Dir, FixedDir: String;
begin
  Result := True;

  if CurPageID = wpSelectDir then
  begin
    Dir := CanonicalDir(WizardDirValue);

    if IsUnderWindowsPath(Dir) then
    begin
      MsgBox(
        '不能安装到 Windows 系统目录：' + #13#10 +
        Dir + #13#10#13#10 +
        '请使用默认目录，或选择例如 D:\Apps\{#AppRootName}。',
        mbError,
        MB_OK
      );
      Result := False;
      Exit;
    end;

    FixedDir := AddAppRootIfBareContainer(Dir);
    if CompareText(FixedDir, Dir) <> 0 then
    begin
      WizardForm.DirEdit.Text := FixedDir;
      Result := False;
      Exit;
    end;
  end;
end;

// --------------------------------------------------------------------------
//  安装前进程清理
//  覆盖安装时，如果启动器或后端 Python 仍在运行，文件可能被占用。
//  这里仅清理当前应用相关的 exe 和 Python 子进程，不再清理 Git 等旧依赖。
// --------------------------------------------------------------------------
procedure KillProcessByImageName(const ImageName: String);
var
  ResultCode: Integer;
begin
  Exec(
    ExpandConstant('{sys}\taskkill.exe'),
    '/F /T /IM ' + ImageName,
    '',
    SW_HIDE,
    ewWaitUntilTerminated,
    ResultCode
  );
end;

procedure StopRunningProcesses;
begin
  KillProcessByImageName('{#AppExeName}');
  KillProcessByImageName('pythonw.exe');
  KillProcessByImageName('python.exe');
  Sleep(1000);
end;

// Inno 在真正复制文件前调用。
// 返回非空字符串会中断安装并展示错误文本。
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';

  if IsUnsafeInstallPath(ExpandConstant('{app}')) then
  begin
    Result :=
      '安装位置不安全：' + ExpandConstant('{app}') + #13#10 +
      '请不要安装到 Windows 目录、Program Files 根目录或磁盘根目录。';
    Exit;
  end;

  StopRunningProcesses;
end;

// --------------------------------------------------------------------------
//  卸载逻辑
//  卸载器会先停止进程，再询问用户是否保留本地数据。
//  选择“是”：保留 {app} 目录，方便下次安装沿用 .venv、log 和后端文件。
//  选择“否”：删除整个安装目录。
// --------------------------------------------------------------------------
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  KeepData: Boolean;
  DesktopPath: String;
begin
  case CurUninstallStep of
    usUninstall:
      StopRunningProcesses;

    usPostUninstall:
      begin
        DesktopPath := ExpandConstant('{autodesktop}\{#AppName}.lnk');
        if FileExists(DesktopPath) then
          DeleteFile(DesktopPath);
        DesktopPath := ExpandConstant('{commondesktop}\{#AppName}.lnk');
        if FileExists(DesktopPath) then
          DeleteFile(DesktopPath);
        DesktopPath := ExpandConstant('{userdesktop}\{#AppName}.lnk');
        if FileExists(DesktopPath) then
          DeleteFile(DesktopPath);

        KeepData :=
          MsgBox(
            '是否保留 {#AppName} 的本地数据和运行环境？' + #13#10 +
            ExpandConstant('{app}') + #13#10#13#10 +
            '点击「是」保留数据，点击「否」删除整个安装目录。',
            mbConfirmation,
            MB_YESNO or MB_DEFBUTTON1
          ) = IDYES;

        if not KeepData then
        begin
          if DirExists(ExpandConstant('{app}')) then
            DelTree(ExpandConstant('{app}'), True, True, True);
          if DirExists(ExpandConstant('{app}')) then
            RemoveDir(ExpandConstant('{app}'));
        end;
      end;
  end;
end;
