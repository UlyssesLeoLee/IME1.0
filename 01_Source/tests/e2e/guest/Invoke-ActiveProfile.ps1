# Guest side: reports which input method profile is active right now, in this session.
# Runs in the interactive session of the logged-on user.
#   powershell -NoProfile -File Invoke-ActiveProfile.ps1 -ParametersPath <file> -ResultPath <file>
# The parameters file holds: clsid (the text service to ask about).
#
# The question is answered with ITfInputProcessorProfiles::GetActiveLanguageProfile, the API Microsoft documents for
# "which profile of this text service is active". It fails when the text service is not the active one, which is
# exactly the signal the cases ST-001..ST-003 need ("PIME is active" / "the original IME is active again").
param(
    [Parameter(Mandatory = $true)][string]$ParametersPath,
    [Parameter(Mandatory = $true)][string]$ResultPath
)
$ErrorActionPreference = 'Stop'
$Parameters = Get-Content -LiteralPath $ParametersPath -Raw -Encoding UTF8 | ConvertFrom-Json
$Output = $null
try {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

[ComImport, Guid("1F02B6C5-7842-4EE6-8A0B-9A24183A95CA"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface ITfInputProcessorProfiles {
    [PreserveSig] int Register([MarshalAs(UnmanagedType.LPStruct)] Guid rclsid);
    [PreserveSig] int Unregister([MarshalAs(UnmanagedType.LPStruct)] Guid rclsid);
    [PreserveSig] int AddLanguageProfile([MarshalAs(UnmanagedType.LPStruct)] Guid rclsid, ushort langid,
        [MarshalAs(UnmanagedType.LPStruct)] Guid guidProfile, [MarshalAs(UnmanagedType.LPWStr)] string desc, uint cchDesc,
        [MarshalAs(UnmanagedType.LPWStr)] string iconFile, uint cchIconFile, uint uiIconIndex);
    [PreserveSig] int RemoveLanguageProfile([MarshalAs(UnmanagedType.LPStruct)] Guid rclsid, ushort langid,
        [MarshalAs(UnmanagedType.LPStruct)] Guid guidProfile);
    [PreserveSig] int EnumInputProcessorProfileInfo(uint index, out IntPtr enumerator);
    [PreserveSig] int GetDefaultLanguageProfile(ushort langid, [MarshalAs(UnmanagedType.LPStruct)] Guid catid,
        out Guid clsid, out Guid guidProfile);
    [PreserveSig] int SetDefaultLanguageProfile(ushort langid, [MarshalAs(UnmanagedType.LPStruct)] Guid rclsid,
        [MarshalAs(UnmanagedType.LPStruct)] Guid guidProfile);
    [PreserveSig] int ActivateLanguageProfile([MarshalAs(UnmanagedType.LPStruct)] Guid rclsid, ushort langid,
        [MarshalAs(UnmanagedType.LPStruct)] Guid guidProfile);
    [PreserveSig] int GetActiveLanguageProfile([MarshalAs(UnmanagedType.LPStruct)] Guid rclsid, out ushort langid,
        out Guid guidProfile);
}

public static class PimeProfiles {
    [DllImport("ole32.dll")]
    static extern int CoCreateInstance([MarshalAs(UnmanagedType.LPStruct)] Guid clsid, IntPtr outer, uint context,
        [MarshalAs(UnmanagedType.LPStruct)] Guid iid, [MarshalAs(UnmanagedType.Interface)] out object result);

    // Returns {"active":true,"profile":"{...}","langid":"0x0804"} or {"active":false,"hresult":"0x80004005"}.
    public static string Query(string clsidText) {
        object instance;
        int hr = CoCreateInstance(new Guid("33C53A50-F456-4884-B049-85FD643ECFED"), IntPtr.Zero, 1,
            new Guid("1F02B6C5-7842-4EE6-8A0B-9A24183A95CA"), out instance);
        if (hr != 0) {
            return "{\"active\":false,\"stage\":\"CoCreateInstance\",\"hresult\":\"0x" + hr.ToString("X8") + "\"}";
        }
        var profiles = (ITfInputProcessorProfiles)instance;
        ushort langid;
        Guid profile;
        hr = profiles.GetActiveLanguageProfile(new Guid(clsidText), out langid, out profile);
        if (hr != 0) {
            return "{\"active\":false,\"stage\":\"GetActiveLanguageProfile\",\"hresult\":\"0x" + hr.ToString("X8") + "\"}";
        }
        return "{\"active\":true,\"profile\":\"" + profile.ToString("B").ToUpper() +
               "\",\"langid\":\"0x" + langid.ToString("X4") + "\"}";
    }
}
'@
    $Output = @{ ok = $true; query = ([PimeProfiles]::Query($Parameters.clsid) | ConvertFrom-Json) }
} catch {
    $Output = @{ ok = $false; error = $_.Exception.Message }
}
$Output | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
