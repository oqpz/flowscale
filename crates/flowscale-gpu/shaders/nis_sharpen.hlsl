// Clean-room 6-tap directional scaling + adaptive unsharp masking (cs_5_0).
cbuffer UpscaleParams : register(b0)
{
    uint2 InputSize;
    uint2 OutputSize;
    uint4 DstRect;
    float Sharpness;
    uint ModeFlags;
    uint Subtype;
    uint VrsFlag;
};

Texture2D<float4> InputTex : register(t0);
RWTexture2D<float4> OutputTex : register(u0);
SamplerState LinearClamp : register(s0);

float Luma(float3 c)
{
    return dot(c, float3(0.2126f, 0.7152f, 0.0722f));
}

float BicubicFilter6(float x)
{
    float ax = abs(x);
    if (ax < 1.0f)
        return (1.25f * ax - 2.25f) * ax * ax + 1.0f;
    if (ax < 2.0f)
        return ((-0.5f * ax + 2.5f) * ax - 4.0f) * ax + 2.0f;
    return 0.0f;
}

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= OutputSize.x || tid.y >= OutputSize.y)
        return;

    if (tid.x < DstRect.x || tid.y < DstRect.y ||
        tid.x >= DstRect.x + DstRect.z || tid.y >= DstRect.y + DstRect.w)
    {
        OutputTex[tid.xy] = float4(0.0f, 0.0f, 0.0f, 1.0f);
        return;
    }

    uint2 localTid = tid.xy - DstRect.xy;
    uint2 dstSize = max(DstRect.zw, uint2(1, 1));

    float2 uv = (float2(localTid) + 0.5f) / float2(dstSize);
    float2 pos = uv * float2(InputSize) - 0.5f;
    int2 baseCoord = int2(floor(pos));
    float2 f = pos - float2(baseCoord);

    int2 c00 = clamp(baseCoord, int2(0, 0), int2(InputSize) - 1);
    int2 c10 = clamp(baseCoord + int2(1, 0), int2(0, 0), int2(InputSize) - 1);
    int2 c01 = clamp(baseCoord + int2(0, 1), int2(0, 0), int2(InputSize) - 1);
    int2 c11 = clamp(baseCoord + int2(1, 1), int2(0, 0), int2(InputSize) - 1);

    float3 p00 = InputTex.Load(int3(c00, 0)).rgb;
    float3 p10 = InputTex.Load(int3(c10, 0)).rgb;
    float3 p01 = InputTex.Load(int3(c01, 0)).rgb;
    float3 p11 = InputTex.Load(int3(c11, 0)).rgb;

    float gx = abs((Luma(p10) + Luma(p11)) - (Luma(p00) + Luma(p01)));
    float gy = abs((Luma(p01) + Luma(p11)) - (Luma(p00) + Luma(p10)));
    bool horizontal = gx >= gy;
    float axisFrac = horizontal ? f.x : f.y;

    float4 baseSample = InputTex.SampleLevel(LinearClamp, uv, 0);

    // 6-tap directional scaling along the dominant axis with bilinear cross-axis filtering.
    float3 dirAccum = 0.0f;
    float wSum = 0.0f;
    int axisBase = horizontal ? baseCoord.x : baseCoord.y;
    int axisMax = (int)(horizontal ? InputSize.x : InputSize.y) - 1;
    float invAxis = 1.0f / float(horizontal ? InputSize.x : InputSize.y);
    [unroll]
    for (int k = -2; k <= 3; ++k)
    {
        float coord = float(clamp(axisBase + k, 0, axisMax));
        float sampleU = (coord + 0.5f) * invAxis;
        float2 tapUv = horizontal ? float2(sampleU, uv.y) : float2(uv.x, sampleU);
        float w = BicubicFilter6(float(k) - axisFrac);
        dirAccum += InputTex.SampleLevel(LinearClamp, tapUv, 0).rgb * w;
        wSum += w;
    }
    float3 dirColor = dirAccum / max(wSum, 1e-5f);

    float3 minC = min(min(p00, p10), min(p01, p11));
    float3 maxC = max(max(p00, p10), max(p01, p11));
    float3 scaled = lerp(baseSample.rgb, clamp(dirColor, minC, maxC), saturate(max(gx, gy) * 3.0f));

    // Adaptive unsharp masking suppressed in flat/noisy areas and clamped to avoid halos.
    if (Sharpness > 0.001f)
    {
        float2 invIn = 1.0f / float2(InputSize);
        float3 n = InputTex.SampleLevel(LinearClamp, uv + float2(0.0f, -invIn.y), 0).rgb;
        float3 s = InputTex.SampleLevel(LinearClamp, uv + float2(0.0f,  invIn.y), 0).rgb;
        float3 w = InputTex.SampleLevel(LinearClamp, uv + float2(-invIn.x, 0.0f), 0).rgb;
        float3 e = InputTex.SampleLevel(LinearClamp, uv + float2( invIn.x, 0.0f), 0).rgb;

        float3 blur = 0.25f * (n + s + w + e);
        float3 detail = scaled - blur;
        float localRange = max(Luma(maxC) - Luma(minC), 1e-4f);
        float edgeGate = saturate(localRange * 4.0f);
        float strength = saturate(Sharpness * 0.1f) * edgeGate;
        scaled = clamp(scaled + detail * strength, minC - abs(minC) * 0.05f, maxC + abs(maxC) * 0.05f);
    }

    OutputTex[tid.xy] = float4(scaled, baseSample.a);
}
