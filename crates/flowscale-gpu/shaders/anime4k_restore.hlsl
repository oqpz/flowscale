// Clean-room Sobel luma-gradient line thinning and restoration shader (cs_5_0).
// Subtype: 0=S, 1=M, 2=L, 3=VL, 4=UL. VrsFlag reduces restoration to one iteration.
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
    float4 center = InputTex.SampleLevel(LinearClamp, uv, 0);

    float2 invIn = 1.0f / float2(InputSize);

    float3 tl = InputTex.SampleLevel(LinearClamp, uv + float2(-invIn.x, -invIn.y), 0).rgb;
    float3 tc = InputTex.SampleLevel(LinearClamp, uv + float2(0.0f,    -invIn.y), 0).rgb;
    float3 tr = InputTex.SampleLevel(LinearClamp, uv + float2( invIn.x, -invIn.y), 0).rgb;
    float3 ml = InputTex.SampleLevel(LinearClamp, uv + float2(-invIn.x,  0.0f), 0).rgb;
    float3 mr = InputTex.SampleLevel(LinearClamp, uv + float2( invIn.x,  0.0f), 0).rgb;
    float3 bl = InputTex.SampleLevel(LinearClamp, uv + float2(-invIn.x,  invIn.y), 0).rgb;
    float3 bc = InputTex.SampleLevel(LinearClamp, uv + float2(0.0f,     invIn.y), 0).rgb;
    float3 br = InputTex.SampleLevel(LinearClamp, uv + float2( invIn.x,  invIn.y), 0).rgb;

    float ltl = Luma(tl);
    float ltc = Luma(tc);
    float ltr = Luma(tr);
    float lml = Luma(ml);
    float lmr = Luma(mr);
    float lbl = Luma(bl);
    float lbc = Luma(bc);
    float lbr = Luma(br);

    // Sobel 3x3 gradient.
    float gx = (ltr + 2.0f * lmr + lbr) - (ltl + 2.0f * lml + lbl);
    float gy = (lbl + 2.0f * lbc + lbr) - (ltl + 2.0f * ltc + ltr);
    float gradMag = sqrt(gx * gx + gy * gy);

    float3 minC = min(min(min(tl, tr), min(bl, br)), min(min(tc, bc), min(ml, mr)));
    float3 maxC = max(max(max(tl, tr), max(bl, br)), max(max(tc, bc), max(ml, mr)));

    if (gradMag < 1e-4f)
    {
        OutputTex[tid.xy] = center;
        return;
    }

    float2 gradDir = float2(gx, gy) / gradMag;
    uint preset = min(Subtype, 4u);
    float stepScale = (0.25f + 0.12f * float(preset)) * (1.0f + saturate(Sharpness * 0.08f));

    // Push line edges inward along the gradient toward crisp dark/light line extrema.
    float3 cur = center.rgb;
    float2 curUv = uv;
    uint iters = (VrsFlag != 0u) ? 1u : min(preset + 1u, 4u);
    float2 stepUv = gradDir * invIn * (0.45f / float(iters));
    [loop]
    for (uint i = 0u; i < iters; ++i)
    {
        float3 posTap = InputTex.SampleLevel(LinearClamp, saturate(curUv + stepUv), 0).rgb;
        float3 negTap = InputTex.SampleLevel(LinearClamp, saturate(curUv - stepUv), 0).rgb;
        bool pickPos = Luma(posTap) < Luma(negTap);
        float3 target = pickPos ? posTap : negTap;
        curUv = saturate(curUv + (pickPos ? stepUv : -stepUv));
        float push = saturate(gradMag * stepScale);
        cur = lerp(cur, target, push * 0.5f);
    }

    OutputTex[tid.xy] = float4(clamp(cur, minC, maxC), center.a);
}
